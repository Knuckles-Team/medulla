"""The set-once holders renamed by MEDULLA-CONTRACT-R063.1 are not census findings."""

from __future__ import annotations

import shutil
import subprocess
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]

# The eleven holder types (old name -> its renamed type), MEDULLA-CONTRACT-R063.1.
RENAMED_HOLDERS = {
    "ResultStatus": "AsrOutcome",
    "ManifestStatus": "ManifestOutcome",
    "JobStatus": "TtsOutcome",
    "GrampsPilotPhase": "GrampsPilotMilestone",
    "ImmichPilotPhase": "ImmichPilotMilestone",
    "TranscriptState": "TranscriptSnapshot",
    "DerivedState": "DerivedValue",
    "ReplayStatus": "ReplayOutcome",
    "IndexFileStatus": "IndexFileVerdict",
    "IndexRefStatus": "IndexRefPresence",
    "TckStatus": "TckVerdict",
    "MappingApprovalState": "MappingApproval",
    "MigrationState": "MigrationOutcome",
}
# Holders of other crates that keep the old name for an unrelated enum.
UNRELATED = {"JobStatus"}  # medulla-quantum-core's provider job status carries data.


# spec: MEDULLA-CONTRACT-R063.1
def test_set_once_holders_are_not_census_findings() -> None:
    hook = shutil.which("pipelines-hook")
    if hook is None:
        pytest.fail("pipelines-hook is required to run the state-machine census")
    run = subprocess.run(
        [hook, "state-machine-census"],
        cwd=ROOT,
        capture_output=True,
        text=True,
        check=False,
        timeout=900,
    )
    report = run.stdout + run.stderr
    for old, new in RENAMED_HOLDERS.items():
        if old in UNRELATED:
            continue
        assert old not in report, (
            f"{old} is still a census finding; rename it to {new}"
        )
