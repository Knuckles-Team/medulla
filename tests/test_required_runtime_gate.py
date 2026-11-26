"""A pull request executes the required runtime manifest (R011.3)."""

from __future__ import annotations

import re
from pathlib import Path

import pytest
import yaml

pytestmark = pytest.mark.no_engine

ROOT = Path(__file__).resolve().parents[1]
WORKFLOW = ROOT / ".github" / "workflows" / "ci.yml"
EXAMPLE = (
    ROOT / "crates" / "medulla-source-types" / "examples" / "print_required_manifest.rs"
)
MANIFEST_SOURCE = (
    ROOT / "crates" / "medulla-source-types" / "src" / "pre_merge_test_module.rs"
)
GATE = "Knuckles-Team/pipelines/.github/workflows/required-runtime.yml@main"


def _doc() -> dict:
    return yaml.safe_load(WORKFLOW.read_text(encoding="utf-8"))


# spec: MEDULLA-DURABLE-KERNEL-R011.3
def test_pull_request_executes_required_categories() -> None:
    doc = _doc()
    triggers = doc[True] if True in doc else doc["on"]
    assert "pull_request" in triggers
    jobs = doc["jobs"]

    # The manifest is the one `required_test_modules` prints, nothing more.
    manifest = jobs["required-runtime-manifest"]
    assert manifest["if"] == "github.event_name == 'pull_request'"
    printer = next(step for step in manifest["steps"] if step.get("id") == "manifest")
    assert (
        "cargo run --locked --quiet -p medulla-source-types "
        "--example print_required_manifest" in printer["run"]
    )
    assert manifest["outputs"]["manifest"] == "${{ steps.manifest.outputs.json }}"
    example = EXAMPLE.read_text(encoding="utf-8")
    assert "required_test_modules()" in example
    assert "println!" in example

    # The required job is the shared gate: a reusable call that adds no shell logic.
    gate = jobs["required-runtime"]
    assert gate["uses"] == GATE
    assert gate["if"] == "github.event_name == 'pull_request'"
    assert gate["needs"] == "required-runtime-manifest"
    assert gate["with"] == {
        "manifest": "${{ needs.required-runtime-manifest.outputs.manifest }}"
    }
    assert "steps" not in gate and "run" not in gate

    # The manifest names the three required categories.
    source = MANIFEST_SOURCE.read_text(encoding="utf-8")
    declared = re.findall(r"\(\s*(Raft|Backup|Persistence),", source)
    assert sorted(set(declared)) == ["Backup", "Persistence", "Raft"]


# spec: MEDULLA-DURABLE-KERNEL-R011.3
def test_failed_or_empty_category_blocks_acceptance() -> None:
    jobs = _doc()["jobs"]
    for name in ("required-runtime-manifest", "required-runtime"):
        job = jobs[name]
        # Nothing may mask a failure: no continue-on-error and no `|| true`.
        assert "continue-on-error" not in job, name
        assert "continue-on-error" not in str(job.get("steps", [])), name
        assert "|| true" not in str(job), name
    assert "needs.required-runtime-manifest.result" not in str(jobs["required-runtime"])
    # An empty selection is refused at the source: a filter that selects no test.
    source = MANIFEST_SOURCE.read_text(encoding="utf-8")
    assert "EmptyTestSelection" in source
    assert "fn validate_selection" in source
    # A declaration the gate cannot select is refused before the manifest is printed.
    example = EXAMPLE.read_text(encoding="utf-8")
    assert "cargo_test_args()" in example and "panic!" in example
