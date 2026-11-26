"""The set-once holders renamed by MEDULLA-CONTRACT-R063.1 are not census findings."""

from __future__ import annotations

import shutil
import subprocess
from pathlib import Path

import pytest
import tomllib
import yaml

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
        [str(hook), "state-machine-census"],
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
        assert old not in report, f"{old} is still a census finding; rename it to {new}"


def _hook_stages(hook_id: str) -> list[list[str]]:
    """The `stages` of every pre-commit hook entry with this id."""
    config = yaml.safe_load((ROOT / ".config" / "pre-commit.yaml").read_text())
    return [
        list(hook.get("stages", []))
        for repo in config["repos"]
        for hook in repo.get("hooks", [])
        if hook.get("id") == hook_id
    ]


# spec: MEDULLA-CONTRACT-R063
@pytest.mark.spec("MEDULLA-CONTRACT-R063")
def test_state_machine_hook_blocks_and_census_is_empty() -> None:
    policy = tomllib.loads((ROOT / "pyproject.toml").read_text())
    machine = policy["tool"]["pipelines_hooks"]["state_machine"]
    assert machine["mode"] == "blocking"
    assert machine["paths"] == ["crates", "medulla", "tools"]

    assert not _hook_stages("state-machine-census"), (
        "the report-mode hook id is replaced by state-machine-shape"
    )
    shape = _hook_stages("state-machine-shape")
    assert len(shape) == 1, "one state-machine-shape hook entry"
    assert set(shape[0]) == {"pre-commit", "pre-push"}

    hook = shutil.which("pipelines-hook")
    if hook is None:
        pytest.fail("pipelines-hook is required to run the state-machine census")
    run = subprocess.run(
        [str(hook), "state-machine-shape"],
        cwd=ROOT,
        capture_output=True,
        text=True,
        check=False,
        timeout=900,
    )
    assert run.returncode == 0, (
        "the census reports findings:\n" + run.stdout + run.stderr
    )
