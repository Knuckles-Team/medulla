"""The workspace-hack gate regenerates and diffs the generated crate.

MEDULLA-CONTRACT-R069.2. The gate is `cargo xtask workspace-hack`. These tests drive
it against a fixture workspace so they do not depend on the real crate set: two
members that use one outside dependency with different feature sets, a generated
hack crate, then a stale one. They need cargo and the pinned `cargo-hakari` and
skip where either is missing; the gate runs on a build host.
"""

from __future__ import annotations

import shutil
import subprocess
from pathlib import Path

import pytest

pytestmark = pytest.mark.no_engine

ROOT = Path(__file__).resolve().parents[1]
PINNED = "0.9.39"


def _tool(
    *args: str, cwd: Path, check: bool = True
) -> subprocess.CompletedProcess[str]:
    proc = subprocess.run(args, cwd=cwd, capture_output=True, text=True)
    if check:
        assert proc.returncode == 0, f"{args}: {proc.stdout}{proc.stderr}"
    return proc


def _need_hakari() -> None:
    if not shutil.which("cargo"):
        pytest.skip("cargo is unavailable here; this gate runs on a build host")
    probe = subprocess.run(
        ["cargo", "hakari", "--version"], capture_output=True, text=True
    )
    if probe.returncode != 0:
        pytest.skip("cargo-hakari is unavailable here; this gate runs on a build host")


def _gate(fixture: Path) -> subprocess.CompletedProcess[str]:
    return _tool(
        "cargo", "run", "-q", "-p", "medulla-xtask", "--",
        "workspace-hack", "--root", str(fixture),
        cwd=ROOT, check=False,
    )  # fmt: skip


def _member(ws: Path, name: str, features: str) -> None:
    crate = ws / "crates" / name
    (crate / "src").mkdir(parents=True)
    (crate / "src" / "lib.rs").write_text("", encoding="utf-8")
    (crate / "Cargo.toml").write_text(
        f'[package]\nname = "{name}"\nversion = "0.0.0"\nedition = "2021"\n'
        "publish = false\n\n"
        f'[dependencies]\nshared = {{ path = "../../../shared", {features} }}\n',
        encoding="utf-8",
    )


@pytest.fixture
def fixture_workspace(tmp_path: Path) -> Path:
    """Two members share one outside dependency with differing features."""
    _need_hakari()
    shared = tmp_path / "shared"
    (shared / "src").mkdir(parents=True)
    (shared / "src" / "lib.rs").write_text("", encoding="utf-8")
    (shared / "Cargo.toml").write_text(
        '[package]\nname = "shared"\nversion = "0.1.0"\nedition = "2021"\n\n'
        "[features]\nf = []\ng = []\n",
        encoding="utf-8",
    )
    ws = tmp_path / "ws"
    ws.mkdir()
    (ws / "Cargo.toml").write_text(
        '[workspace]\nresolver = "2"\nmembers = ["crates/*"]\n', encoding="utf-8"
    )
    _member(ws, "left", "features = []")
    _member(ws, "right", 'features = ["f"]')
    _tool("cargo", "hakari", "init", "crates/fixture-hack", "--yes", cwd=ws)
    _tool("cargo", "hakari", "generate", cwd=ws)
    _tool("cargo", "hakari", "manage-deps", "--yes", cwd=ws)
    return ws


# spec: MEDULLA-CONTRACT-R069.2
def test_generated_crate_matches_a_fresh_generation(fixture_workspace: Path) -> None:
    fresh = _gate(fixture_workspace)
    assert fresh.returncode == 0, fresh.stdout + fresh.stderr

    # A member asks for a feature the checked-in crate lacks: the gate must fail
    # and show the diff.
    right = fixture_workspace / "crates" / "right" / "Cargo.toml"
    right.write_text(
        right.read_text(encoding="utf-8").replace(
            'features = ["f"]', 'features = ["f", "g"]'
        ),
        encoding="utf-8",
    )
    stale = _gate(fixture_workspace)
    assert stale.returncode != 0
    assert "stale" in stale.stderr
    assert '"g"' in stale.stderr, stale.stderr


# spec: MEDULLA-CONTRACT-R069.2
def test_a_different_tool_version_is_refused() -> None:
    _need_hakari()
    version = subprocess.run(
        ["cargo", "hakari", "--version"], capture_output=True, text=True
    ).stdout.split()[-1]
    if version != PINNED:
        gate = _gate(ROOT)
        assert gate.returncode != 0
        assert "HAKARI_VERSION_MISMATCH" in gate.stderr
    else:
        pytest.skip("the tool is the pinned version; the Rust unit test covers refusal")


def test_the_pin_is_one_value_everywhere() -> None:
    boot = (ROOT / "scripts" / "bootstrap.sh").read_text(encoding="utf-8")
    gate = (ROOT / "tools" / "xtask" / "src" / "workspace_hack.rs").read_text(
        encoding="utf-8"
    )
    assert f"hakari_version={PINNED}" in boot
    assert f'PINNED_VERSION: &str = "{PINNED}"' in gate
    assert "HAKARI_VERSION_MISMATCH" in gate
