"""Rust wire types and the hand-kept `medulla/generated` mirror must not drift."""

from __future__ import annotations

import subprocess
import sys
from pathlib import Path

import pytest

pytestmark = pytest.mark.no_engine

ROOT = Path(__file__).resolve().parents[1]
CHECK = ROOT / "scripts" / "check_generated_parity.py"


def _run(root: Path) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, str(CHECK), str(root)],
        capture_output=True,
        text=True,
        timeout=300,
        check=False,
    )


def test_generated_python_matches_rust_wire_types() -> None:
    result = _run(ROOT)
    assert result.returncode == 0, result.stdout + result.stderr


def test_gate_fails_on_unexplained_drift(tmp_path: Path) -> None:
    crate = tmp_path / "crates" / "demo" / "src"
    crate.mkdir(parents=True)
    (crate / "lib.rs").write_text(
        "#[derive(Serialize, Deserialize)]\n"
        "pub struct Probe {\n    pub alpha: u32,\n    pub beta: u32,\n}\n"
    )
    generated = tmp_path / "medulla" / "generated"
    generated.mkdir(parents=True)
    (generated / "demo.py").write_text(
        "from pydantic import BaseModel\n\n\nclass Probe(BaseModel):\n    alpha: int\n"
    )
    result = _run(tmp_path)
    assert result.returncode == 1
    assert "DRIFT Probe: missing field beta" in result.stdout
    # Every allowlist entry is stale against a tree that has none of its types.
    assert "STALE" in result.stdout
