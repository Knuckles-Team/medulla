"""MEDULLA-DECISION-ENGINE-R711.7 and MEDULLA-IDENTITY-R738 wire mirrors.

L1: the op builders of ``medulla.decision_client`` and the generated models agree
with the engine's wire shapes, with no engine running.
"""

from __future__ import annotations

import pytest
from pydantic import TypeAdapter, ValidationError

from medulla import decision_client as dc
from medulla.generated import models

CASE = {
    "case_id": "case-1",
    "skill_ref": "skill:triage@3",
    "kind": "golden",
    "input_digest": "in-1",
    "expected_digest": "out-1",
    "source": "authored",
}
CALIBRATION = {
    "judge_ref": "judge-1",
    "annotated_set_ref": "set-1",
    "kappa_ppm": 700_000,
    "n": 60,
}


# spec: MEDULLA-DECISION-ENGINE-R711.7
@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R711.7")
def test_case_report_calibrate_ops_validate_against_the_generated_models() -> None:
    # spec: MEDULLA-DECISION-ENGINE-R711.7
    adapter = TypeAdapter(models.DecisionEvalOp)
    ops = [
        dc.skill_eval_case_add_op(
            {"tenant_id": "t", "case": CASE, "known_tools": ["search"]}
        ),
        dc.skill_eval_report_op("t", "skill:triage@3"),
        dc.skill_eval_calibrate_op("t", "skill:triage@3", CALIBRATION),
    ]
    assert [op["op"] for op in ops] == [
        "skill_eval_case_add",
        "skill_eval_report",
        "skill_eval_calibrate",
    ]
    for op in ops:
        adapter.validate_python(op)
    # A field the engine does not know is refused, as its deny_unknown_fields does.
    bad = dc.skill_eval_report_op("t", "skill:triage@3")
    bad["request"]["extra"] = 1
    with pytest.raises(ValidationError):
        adapter.validate_python(bad)


# spec: MEDULLA-DECISION-ENGINE-R712.3
@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R712.3")
def test_ablation_and_canary_are_the_learn_writes_of_the_one_route() -> None:
    # spec: MEDULLA-DECISION-ENGINE-R712.3
    ablation = dc.record_ablation_op(
        "t", {"skill_ref": "skill:a@1", "identical_scores_ppm": [], "runs": []}
    )
    assert ablation["op"] == "learn"
    assert ablation["write"]["write"] == "record_ablation"
    canary = dc.record_canary_op(
        "t",
        {
            "skill_ref": "skill:a@1",
            "champion_revision": "r1",
            "challenger_revision": "r2",
            "traffic_ppm": 100_000,
            "window_ms": 3_600_000,
        },
        "ramp",
    )
    assert canary["write"]["write"] == "record_canary"
    assert canary["write"]["verdict"] == "ramp"


# spec: MEDULLA-IDENTITY-R738
@pytest.mark.spec("MEDULLA-IDENTITY-R738")
def test_a_person_event_carries_no_principal_pseudonym() -> None:
    # spec: MEDULLA-IDENTITY-R738
    event = {
        "invocation_id": "i1",
        "tenant_id": "t",
        "skill_ref": "skill:a@1",
        "skill_version": "1",
        "chosen_by": "agent",
        "run_id": "r1",
        "started_ms": 1,
        "ended_ms": 2,
        "outcome": "ok",
        "tokens_in": 1,
        "tokens_out": 1,
    }
    op = dc.skill_telemetry_ingest_op("__commons__", [event])
    assert "principal_pseudonym" not in op["events"][0]
    # A service computes the 64-hex pseudonym itself.
    assert len(dc.skill_pseudonym("t", "svc")) == 64
