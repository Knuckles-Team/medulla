"""Topology contract for the central reusable lightweight-gates caller."""

from __future__ import annotations

from pathlib import Path

import pytest
import yaml

pytestmark = pytest.mark.no_engine

ROOT = Path(__file__).resolve().parents[1]
WORKFLOW = ROOT / ".github/workflows/ci.yml"
CENTRAL = "Knuckles-Team/pipelines/.github/workflows/medulla_light_gates.yml@main"
RUNNER = (
    "${{ vars.ARBORA_TRUSTED_CI_RUNNER == 'true' && "
    "'\"arbora-ci-trusted\"' || '\"ubuntu-latest\"' }}"
)

# Every job that can run on a pull request. Main-only jobs fail the workflow
# run release.yml requires, not the branch-protection aggregate.
REQUIRED_BLOCKING_JOBS = [
    "pr-scope",
    "ci",
    "light-gates",
    "changes",
    "security",
    "gates-wheel",
    "gates-whisper",
    "gates-ontology",
    "gates-crates",
    "gates-storage",
    "gates-pyo3",
    "python-kernels",
    "python-server",
    "python-suite",
    "no-auth-uds",
    "production-upgrade",
    "tts-piper-inference",
    "language-clients",
    "quality-advisory",
]
# Gates the aggregate requires only on the full matrix (nightly, `v*` tag,
# manual dispatch): a pull request and a push to main need only `ci`.
FULL = (
    "(github.event_name == 'schedule' || github.event_name == 'workflow_dispatch' "
    "|| startsWith(github.ref, 'refs/tags/v'))"
)
MAIN_PUSH_REQUIRED_JOBS = {"no-auth-uds", "production-upgrade"}
MAIN_ONLY_JOBS = {
    "gates",
    "gates-plan-fuzz",
    "gates-facade",
    "gates-variants",
    "clippy-release-profiles",
}
PATH_GATED_JOBS = {
    "gates-wheel": "packaging",
    "gates-pyo3": "packaging",
    "gates-whisper": "whisper",
    "gates-ontology": "ontology",
    "tts-piper-inference": "tts",
    "language-clients": "clients",
}
NON_BLOCKING_JOBS = {
    *MAIN_ONLY_JOBS,
    "feature-matrix",
    "benchmarks",
    "wheel-cache",
    "engine-main-wheel",
}


def _jobs() -> dict:
    return yaml.safe_load(WORKFLOW.read_text(encoding="utf-8"))["jobs"]


def _needs(job: dict) -> set[str]:
    value = job.get("needs", [])
    return {value} if isinstance(value, str) else set(value)


def test_light_gates_is_the_single_central_light_gate_call() -> None:
    jobs = _jobs()
    assert jobs["light-gates"] == {
        "if": "github.event_name != 'pull_request'",
        "uses": CENTRAL,
        "with": {"runner": RUNNER},
    }
    # Native jobs read scope from the local job, never from the reusable
    # call, so they do not wait behind the central pre-commit leg.
    assert "uses" not in jobs["changes"]
    for name, job in jobs.items():
        if name not in {"ci-required", "engine-main-wheel"}:
            assert "light-gates" not in _needs(job), name
    assert "lint-and-architecture" not in jobs
    assert "spec-gates" not in jobs


def test_all_native_scope_consumers_keep_the_changes_output_edge() -> None:
    jobs = _jobs()
    expected = {
        "security",
        "gates",
        "gates-plan-fuzz",
        "gates-wheel",
        "gates-facade",
        "gates-whisper",
        "gates-ontology",
        "gates-variants",
        "gates-crates",
        "gates-storage",
        "gates-pyo3",
        "python-kernels",
        "python-server",
        "python-suite",
        "no-auth-uds",
        "production-upgrade",
        "tts-piper-inference",
        "language-clients",
        "quality-advisory",
        "clippy-release-profiles",
        "engine-main-wheel",
    }
    actual = {
        name
        for name, job in jobs.items()
        if name != "ci-required" and "changes" in _needs(job)
    }
    assert actual == expected
    for name in expected:
        assert "needs.changes.outputs.code" in str(
            jobs[name].get("if", "")
        ) or name in {
            "security",
            "engine-main-wheel",
        }


def test_main_only_and_path_gated_jobs_skip_on_pull_requests() -> None:
    jobs = _jobs()
    for name in MAIN_ONLY_JOBS | MAIN_PUSH_REQUIRED_JOBS:
        assert jobs[name]["if"] == (
            f"needs.changes.outputs.code == 'true' && {FULL}"
        ), name
    for name, gate in PATH_GATED_JOBS.items():
        assert jobs[name]["if"] == (
            "needs.changes.outputs.code == 'true' && "
            f"({FULL} || "
            f"needs.changes.outputs.{gate} == 'true')"
        ), name
    assert "scanner-quality" not in jobs


def test_publication_waits_for_the_light_gate_call_without_deleted_job_names() -> None:
    jobs = _jobs()
    needs = _needs(jobs["engine-main-wheel"])
    assert {"light-gates", "changes"} <= needs
    assert MAIN_ONLY_JOBS <= needs
    assert "lint-and-architecture" not in needs
    assert "spec-gates" not in needs
    for job in jobs.values():
        assert "lint-and-architecture" not in _needs(job)
        assert "spec-gates" not in _needs(job)


def test_ci_required_is_the_fail_closed_branch_protection_aggregate() -> None:
    jobs = _jobs()
    required = jobs["ci-required"]
    assert required["name"] == "CI required"
    assert required["if"] == "always()"
    assert required["runs-on"] == "ubuntu-latest"
    assert required["needs"] == REQUIRED_BLOCKING_JOBS
    assert not NON_BLOCKING_JOBS & set(required["needs"])

    step = required["steps"][0]
    assert step["name"] == "Require every blocking CI result"
    assert step["env"]["CODE_SCOPE"] == "${{ needs.changes.outputs.code }}"
    for job_name in REQUIRED_BLOCKING_JOBS:
        env_name = job_name.upper().replace("-", "_") + "_RESULT"
        assert step["env"][env_name] == f"${{{{ needs.{job_name}.result }}}}"

    run = step["run"]
    assert "set -euo pipefail" in run
    assert 'result="${result:-MISSING}"' in run
    assert '[[ "$result" == success ]]' in run
    assert '[[ "$CODE_SCOPE" == false ]]' in run
    for job_name, gate in PATH_GATED_JOBS.items():
        assert f'[{job_name}]="${gate.upper()}_SCOPE"' in run
        assert step["env"][f"{gate.upper()}_SCOPE"] == (
            f"${{{{ needs.changes.outputs.{gate} }}}}"
        )
    assert '"$EVENT_NAME" == pull_request' in run
    for job_name in MAIN_PUSH_REQUIRED_JOBS:
        env_name = job_name.upper().replace("-", "_") + "_RESULT"
        assert f'results+=("{job_name}=${env_name}")' in run
    assert "accepted path-gate skip" in run
    assert "expected skipped for documentation-only scope" in run
    assert "failures=()" in run
    assert "exit 1" in run


def test_pr_scope_skips_ci_only_for_an_owner_local_gate_attestation() -> None:
    jobs = _jobs()
    scope = jobs["pr-scope"]
    assert scope["permissions"]["statuses"] == "read"
    attest = scope["steps"][0]
    assert attest["id"] == "attest"
    assert attest["env"]["ATTESTERS"] == "${{ vars.MEDULLA_TRAIN_ATTESTERS }}"
    assert '.context == "CI required"' in attest["run"]
    assert 'startswith("local-ci:")' in attest["run"]
    assert ".creator.login" in attest["run"]
    assert scope["outputs"]["attested"] == "${{ steps.attest.outputs.attested }}"
    assert "needs.pr-scope.outputs.attested != 'true'" in jobs["ci"]["if"]
    assert "pr-scope" in _needs(jobs["ci-required"])
    run = jobs["ci-required"]["steps"][0]["run"]
    assert '"$ATTESTED" == true && "$CI_RESULT" == skipped' in run


def test_main_push_runs_only_ci_and_light_gates() -> None:
    doc = yaml.safe_load(WORKFLOW.read_text(encoding="utf-8"))
    triggers = doc[True] if True in doc else doc["on"]
    assert triggers["push"] == {"branches": ["main"], "tags": ["v*"]}
    jobs = doc["jobs"]
    assert "github.ref == 'refs/heads/main'" in jobs["ci"]["if"]
    assert jobs["light-gates"]["if"] == "github.event_name != 'pull_request'"
    for name, job in jobs.items():
        if name in {"pr-scope", "ci", "ci-required", "light-gates"}:
            continue
        assert "github.event_name == 'push' &&" not in str(job.get("if", "")), name


def test_closed_pull_request_cancels_its_runs() -> None:
    doc = yaml.safe_load(
        (WORKFLOW.parent / "cancel-closed-pr-runs.yml").read_text(encoding="utf-8")
    )
    triggers = doc[True] if True in doc else doc["on"]
    assert triggers == {"pull_request": {"types": ["closed"]}}
    assert doc["permissions"] == {"actions": "write", "contents": "read"}
    assert "/cancel" in str(doc["jobs"]["cancel"]["steps"])
