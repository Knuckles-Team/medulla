"""Task state derived from task records (MEDULLA-SDLC-R005.7).

L3: the engine derives each task state from its Task record (R014.1). The
served read is `derive_task_state`, beside `derive_spec_status` in
`crates/medulla-core/src/lifecycle/status.rs`. The state is derived, never
stored in any file. A task is `done` when its box is ticked on the
default-branch tree, `blocked` when its row has an unresolved `dependsOn`
target or is touched by an unresolved contract (R014.4), and `open`
otherwise. This test drives the engine derivation through the scoped crate
test and asserts the state follows the checkbox and the blockers.
"""

from __future__ import annotations

import os
import re
import subprocess
from pathlib import Path

import pytest

pytestmark = (
    pytest.mark.no_engine
)  # The derivation runs in the scoped crate test, not the shared engine.

ROOT = Path(__file__).resolve().parents[2]
STATUS_RS = ROOT / "crates" / "medulla-core" / "src" / "lifecycle" / "status.rs"
DERIVE_TEST = "lifecycle::status::tests::task_state_follows_checkbox_and_blockers"


def _run_engine_derivation() -> dict[str, str]:
    """Run the engine's task-state derivation and read back the states it reports."""
    proc = subprocess.run(
        [
            "cargo",
            "test",
            "-p",
            "medulla-core",
            "--lib",
            "--",
            "--exact",
            DERIVE_TEST,
            "--nocapture",
        ],
        cwd=ROOT,
        env={**os.environ, "CARGO_TERM_COLOR": "never"},
        capture_output=True,
        text=True,
        timeout=1800,
        check=False,
    )
    if proc.returncode != 0:
        raise AssertionError(
            f"engine task-state derivation test failed:\n{proc.stdout}\n{proc.stderr}"
        )
    states = dict(
        re.findall(r"^TASK_STATE\t(\S+)\t(\S+)$", proc.stdout, flags=re.MULTILINE)
    )
    if len(states) != 6:
        raise AssertionError(f"expected 6 task states, engine reported: {states}")
    return states


def test_task_state_follows_checkbox_and_blockers() -> None:
    source = STATUS_RS.read_text(encoding="utf-8")
    # The derivation lives beside the spec-status derivation in the same
    # module and stores no state of its own.
    assert "pub fn derive_task_state" in source
    assert "pub fn derive_spec_status" in source

    states = _run_engine_derivation()

    # spec: MEDULLA-SDLC-R005.7
    assert states == {
        "T001": "OPEN",  # open box, no unresolved targets
        "T002": "DONE",  # ticked box on the default-branch tree
        "T003": "BLOCKED",  # unresolved dependsOn target + unresolved contract
        "T004": "OPEN",  # dependency resolves to a row: not a blocker
        "T005": "DONE",  # a ticked box wins over its row's blockers
        "T006": "OPEN",  # a row-less task with an open box
    }
