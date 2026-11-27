"""MEDULLA-CONTRACT-R065.7: `Cargo.lock` is current.

A manifest change that forgets the lock must fail at push, not in a later build.
The test resolves the workspace with `--locked` and requires exit 0 and a
byte-identical lock.
"""

from __future__ import annotations

import hashlib
import json
import shutil
import subprocess
from pathlib import Path

import pytest

pytestmark = pytest.mark.no_engine

ROOT = Path(__file__).resolve().parents[1]


# spec: MEDULLA-CONTRACT-R065.7
def test_locked_metadata_resolve_succeeds_on_a_clean_checkout() -> None:
    if shutil.which("cargo") is None:
        pytest.skip("cargo is not on PATH; this gate runs on a build host")
    lock = ROOT / "Cargo.lock"
    before = hashlib.sha256(lock.read_bytes()).hexdigest()
    proc = subprocess.run(
        ["cargo", "metadata", "--locked", "--format-version", "1"],
        cwd=ROOT,
        capture_output=True,
        text=True,
    )
    assert proc.returncode == 0, (
        "Cargo.lock is not current; run `cargo update --workspace` and commit it:\n"
        + proc.stderr[-2000:]
    )
    json.loads(proc.stdout)
    assert hashlib.sha256(lock.read_bytes()).hexdigest() == before, (
        "cargo rewrote Cargo.lock under --locked"
    )
