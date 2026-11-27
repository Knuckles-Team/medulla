"""MEDULLA-CONTRACT-R065.6: the dependency advisory gate passes without new ignores.

`zip` and `url` are on releases past their advisories, so `cargo deny check
advisories` exits 0 and `.config/deny.toml` holds no ignore that the merge base did
not hold.
"""

from __future__ import annotations

import shutil
import subprocess
from pathlib import Path

import pytest
import tomllib

pytestmark = pytest.mark.no_engine

ROOT = Path(__file__).resolve().parents[1]
DENY = ROOT / ".config" / "deny.toml"


def _ignore_ids(text: str) -> set[str]:
    return {e["id"] for e in tomllib.loads(text)["advisories"].get("ignore", [])}


def _base_ignore_ids() -> set[str] | None:
    try:
        base = subprocess.run(
            ["bash", str(ROOT / "scripts" / "gate_base_ref.sh")],
            cwd=ROOT,
            check=True,
            capture_output=True,
            text=True,
        ).stdout.strip()
        shown = subprocess.run(
            ["git", "show", f"{base}:.config/deny.toml"],
            cwd=ROOT,
            check=True,
            capture_output=True,
            text=True,
        ).stdout
    except (subprocess.CalledProcessError, FileNotFoundError):
        return None
    return _ignore_ids(shown)


# spec: MEDULLA-CONTRACT-R065.6
def test_cargo_deny_advisories_pass_without_new_ignores() -> None:
    base = _base_ignore_ids()
    if base is not None:
        added = _ignore_ids(DENY.read_text(encoding="utf-8")) - base
        assert not added, f"new advisory ignores since the merge base: {sorted(added)}"
    if shutil.which("cargo") is None or shutil.which("cargo-deny") is None:
        pytest.skip("cargo-deny is not installed; this gate runs on a build host")
    proc = subprocess.run(
        ["cargo", "deny", "--all-features", "check", "advisories"],
        cwd=ROOT,
        capture_output=True,
        text=True,
    )
    assert proc.returncode == 0, proc.stderr[-3000:]
