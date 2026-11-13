"""The unified planner's falsification gate (MEDULLA-FEDERATED-QUERY-R730).

The bench ``benches/unified_vs_contracts.rs`` produces the report; these tests judge it
and record the freeze decision in the served engine through the one state machine.
"""

from __future__ import annotations

import pytest

PASSING = {
    "arm_a_p95_ms": 6.0,
    "arm_b_p95_ms": 10.0,
    "arm_a_recall": 1.0,
    "arm_b_recall": 1.0,
    "writes_lost": 0,
    "max_p95_ratio": 0.7,
}


@pytest.fixture
def gate(load_script):
    return load_script("unified_planner_gate")


@pytest.mark.no_engine
@pytest.mark.parametrize(
    ("change", "fragment"),
    [
        ({"arm_a_p95_ms": 7.5}, "exceeds"),
        ({"arm_a_recall": 0.9}, "recall differs"),
        ({"writes_lost": 2}, "writes lost"),
    ],
)
def test_each_rule_fails_the_gate(gate, change, fragment):
    verdict = gate.evaluate({**PASSING, **change})
    assert not verdict.passed
    assert any(fragment in reason for reason in verdict.reasons)


@pytest.mark.no_engine
def test_boundary_ratio_passes_and_incomplete_report_is_refused(gate):
    assert gate.evaluate({**PASSING, "arm_a_p95_ms": 7.0}).passed
    with pytest.raises(ValueError, match="PLANNER_GATE_REPORT_INCOMPLETE"):
        gate.evaluate({"arm_a_p95_ms": 1.0})


@pytest.mark.no_engine
def test_every_legal_transition_and_every_illegal_one(gate):
    scope, event = gate.PlannerScope, gate.GateEvent
    assert gate.transition(scope.CLEAR, event.GATE_FAILED) is scope.FROZEN
    assert gate.transition(scope.FROZEN, event.GATE_FAILED) is scope.FROZEN
    assert gate.transition(scope.FROZEN, event.GATE_PASSED) is scope.CLEAR
    assert gate.transition(scope.CLEAR, event.GATE_PASSED) is scope.CLEAR
    with pytest.raises(gate.IllegalTransition, match="PlannerScopeMachine"):
        gate.transition(scope.CLEAR, "reopened")
    with pytest.raises(gate.IllegalTransition, match="PlannerScopeMachine"):
        gate.transition("paused", event.GATE_PASSED)


def test_failed_gate_records_the_freeze_decision(clean_graph, gate):
    assert gate.current_state(clean_graph) is gate.PlannerScope.CLEAR
    assert gate.landing_refusal(clean_graph, "NewOperator") is None

    failed = {**PASSING, "arm_a_p95_ms": 9.0}
    assert gate.record(clean_graph, failed) is gate.PlannerScope.FROZEN
    decided = clean_graph.query.uql(
        f"MATCH (:{gate.RUN_LABEL}) WHERE decision = '{gate.DECISION_CODE}'"
    )
    assert len(decided["rows"]) == 1
    refusal = gate.landing_refusal(clean_graph, "NewOperator")
    assert refusal is not None and refusal.startswith(gate.DECISION_CODE)

    assert gate.record(clean_graph, PASSING) is gate.PlannerScope.CLEAR
    assert gate.landing_refusal(clean_graph, "NewOperator") is None
