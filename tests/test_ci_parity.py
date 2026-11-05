"""MEDULLA-SDLC-R008: `scripts/ci_parity.sh` fails when any one stage fails."""

from __future__ import annotations

import os
import subprocess
from pathlib import Path

import pytest

pytestmark = pytest.mark.no_engine

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "scripts" / "ci_parity.sh"

STUB = """#!/usr/bin/env bash
stage=commit
for arg in "$@"; do
  case "$prev" in --hook-stage) stage="$arg" ;; esac
  prev="$arg"
done
echo "$stage" >> "$STUB_LOG"
if [ "$stage" = "$STUB_FAIL_STAGE" ]; then exit 1; fi
exit 0
"""


def _run(tmp_path: Path, fail_stage: str) -> tuple[int, list[str]]:
    bin_dir = tmp_path / "bin"
    bin_dir.mkdir(exist_ok=True)
    stub = bin_dir / "pre-commit"
    stub.write_text(STUB)
    stub.chmod(0o755)
    log = tmp_path / f"stages-{fail_stage or 'none'}.log"
    env = dict(os.environ)
    env["PATH"] = f"{bin_dir}{os.pathsep}{env.get('PATH', '')}"
    env["STUB_LOG"] = str(log)
    env["STUB_FAIL_STAGE"] = fail_stage
    proc = subprocess.run(
        ["bash", str(SCRIPT)], env=env, capture_output=True, text=True, check=False
    )
    return proc.returncode, log.read_text().split()


@pytest.mark.parametrize("fail_stage", ["", "commit", "pre-push", "manual"])
def test_parity_exit_follows_every_stage(tmp_path: Path, fail_stage: str) -> None:
    code, stages = _run(tmp_path, fail_stage)
    assert stages == ["commit", "pre-push", "manual"]
    if fail_stage:
        assert code != 0
    else:
        assert code == 0
