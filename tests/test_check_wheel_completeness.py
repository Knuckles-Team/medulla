"""The wheel-completeness gate refuses a wheel carrying a console script.

The spec row names the retired Python path ``scripts/check_wheel_completeness.py``;
the gate it describes now lives in ``tools/xtask/src/wheel_completeness.rs``
(``cargo xtask wheel-completeness``). This test drives that gate against a wheel
composed here: a fully folded wheel passes, and the same wheel plus one
``*.data/scripts/`` member is refused, the console script being the only failure.
"""

from __future__ import annotations

import base64
import hashlib
import shutil
import subprocess
import zipfile
from pathlib import Path

import pytest

pytestmark = pytest.mark.no_engine

ROOT = Path(__file__).resolve().parents[1]
VERSION = "9.9.9"
DIST_INFO = f"medulla_engine-{VERSION}.dist-info"
RECORD = f"{DIST_INFO}/RECORD"


def _digest(payload: bytes) -> str:
    raw = hashlib.sha256(payload).digest()
    return "sha256=" + base64.urlsafe_b64encode(raw).decode().rstrip("=")


def _metadata() -> bytes:
    return (
        f"Metadata-Version: 2.3\nName: medulla\nVersion: {VERSION}\n"
        "Requires-Dist: msgpack>=1.2.1\n"
        "Requires-Dist: pyoxigraph>=0.3.22 ; extra == 'owl'\n"
        "Requires-Python: >=3.10\n"
    ).encode()


def _complete_members(console_script: str | None = None) -> dict[str, bytes]:
    """One fully folded wheel; optionally plus the retired console-script layout."""

    members = {
        "medulla/__init__.py": b"",
        "medulla/client.py": b"",
        "medulla/numeric.abi3.so": b"\x7fELF-kernel",
        "medulla/kvcache/__init__.py": b"",
        "medulla/skills/kg-modality-sql/SKILL.md": b"# skill",
        f"{DIST_INFO}/METADATA": _metadata(),
        f"{DIST_INFO}/WHEEL": b"Wheel-Version: 1.0\n",
    }
    if console_script is not None:
        members[console_script] = b"\x7fELF-server"
    return members


def _wheel(tmp_path: Path, members: dict[str, bytes]) -> Path:
    wheel = tmp_path / f"medulla_engine-{VERSION}-py3-none-linux_x86_64.whl"
    rows = [
        f"{name},{_digest(payload)},{len(payload)}"
        for name, payload in sorted(members.items())
    ]
    rows.append(f"{RECORD},,")
    with zipfile.ZipFile(wheel, "w", zipfile.ZIP_DEFLATED) as archive:
        for name, payload in sorted(members.items()):
            archive.writestr(name, payload)
        archive.writestr(RECORD, ("\n".join(rows) + "\n").encode())
    return wheel


def _gate(wheel: Path) -> subprocess.CompletedProcess[str]:
    if shutil.which("cargo") is None:
        pytest.skip("cargo is needed to run the wheel-completeness gate")
    return subprocess.run(
        ["cargo", "--locked", "xtask", "wheel-completeness", str(wheel)],
        cwd=ROOT,
        capture_output=True,
        text=True,
        check=False,
    )


def _failures(done: subprocess.CompletedProcess[str]) -> list[str]:
    return [line[4:] for line in done.stdout.splitlines() if line.startswith("  - ")]


def test_a_clean_folded_wheel_passes(tmp_path: Path) -> None:
    done = _gate(_wheel(tmp_path, _complete_members()))
    assert done.returncode == 0, done.stdout + done.stderr
    assert _failures(done) == []


@pytest.mark.spec("MEDULLA-CONTRACT-R053.2")
def test_a_wheel_carrying_a_console_script_is_rejected(tmp_path: Path) -> None:
    script = f"medulla_engine-{VERSION}.data/scripts/medulla-server"
    done = _gate(_wheel(tmp_path, _complete_members(script)))
    assert done.returncode != 0
    failures = _failures(done)
    assert len(failures) == 1, failures
    assert script in failures[0]
    assert "is a console script" in failures[0]
    assert "medulla/_bin/medulla-server" in failures[0]
