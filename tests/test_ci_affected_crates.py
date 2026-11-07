"""Pull-request affected-crate selection wiring (spec MEDULLA-CONTRACT-R053.7).

The selection itself is `cargo xtask ci-affected-crates`; its cases are Rust
tests in tools/xtask.
"""

from __future__ import annotations

import pathlib

import pytest
import yaml

ROOT = pathlib.Path(__file__).resolve().parents[1]
pytestmark = pytest.mark.no_engine


@pytest.mark.spec("MEDULLA-CONTRACT-R053.7")
def test_pull_request_runs_one_gate() -> None:
    ci = yaml.safe_load((ROOT / ".github/workflows/ci.yml").read_text(encoding="utf-8"))
    jobs = ci["jobs"]
    assert jobs["ci"]["name"] == "CI"
    assert jobs["ci"]["if"] == "github.event_name == 'pull_request'"
    assert "ci" in jobs["ci-required"]["needs"]
    for name in ("light-gates", "changes"):
        assert jobs[name]["if"] == "github.event_name != 'pull_request'"
    assert "pull_request" in jobs["security"]["if"]
    script = "\n".join(s.get("run", "") for s in jobs["ci"]["steps"])
    assert "xtask ci-affected-crates" in script
    assert "--from-ref" in script and "spec-trailers" in script
    assert "cargo nextest run" in script and "cargo clippy" in script
    runs_on = jobs["ci"]["runs-on"]
    for needle in (
        "MEDULLA_WARM_CI_RUNNER",
        "head.repo.full_name",
        "medulla-warm",
        "dependabot[bot]",
        "'ubuntu-24.04'",
    ):
        assert needle in runs_on
    assert "arbora-ci-trusted" not in runs_on
    assert "SCCACHE_GHA_ENABLED=false" in script and "CARGO_TARGET_DIR=" in script


@pytest.mark.spec("MEDULLA-CONTRACT-R053.7.2")
def test_pr_spec_only_decision_uses_the_crate_selection() -> None:
    ci = yaml.safe_load((ROOT / ".github/workflows/ci.yml").read_text(encoding="utf-8"))
    for job in ("pr-scope", "ci"):
        script = "\n".join(s.get("run", "") for s in ci["jobs"][job]["steps"])
        assert "xtask ci-affected-crates" in script
    steps = {s.get("id"): s for s in ci["jobs"]["ci"]["steps"]}
    assert "spec-scope" in steps
    assert "steps.spec-scope.outputs.scope" in steps["crates"]["if"]
