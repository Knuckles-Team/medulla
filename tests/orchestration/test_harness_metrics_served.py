"""MEDULLA-ORCH-VOCAB-R010.1 to R010.5 served harness metric statements: the
run-trace fixtures are ingested through ``add_node`` and every statement of
``HARNESS_QUERIES`` runs through the served ``Method::Uql`` path."""

from __future__ import annotations

import json
from pathlib import Path

import pytest

FIXTURES = (
    Path(__file__).resolve().parents[2]
    / "crates"
    / "medulla-core"
    / "tests"
    / "fixtures"
    / "run_traces.json"
)


def _ingest(client, traces: list[dict] | None = None) -> None:
    for trace in traces or json.loads(FIXTURES.read_text())["traces"]:
        props = {key: value for key, value in trace.items() if key != "id"}
        props["node_type"] = "RunTrace"
        client.nodes.add(trace["id"], props)


def _by_group(result: dict) -> dict[str, dict]:
    assert result["kind"] == "rows"
    return {row["id"]: row["channels"] for row in result["rows"]}


@pytest.mark.spec("MEDULLA-ORCH-VOCAB-R010.1")
def test_success_rate_per_model_and_task_class(clean_graph):
    _ingest(clean_graph)
    groups = _by_group(clean_graph.query.uql("harness_success_rate"))
    assert groups["model-a|shell-task"]["success_rate"] == pytest.approx(0.5)
    assert groups["model-a|edit-task"]["success_rate"] == pytest.approx(0.0)
    assert groups["model-b|shell-task"]["success_rate"] == pytest.approx(0.0)
    assert groups["model-b|edit-task"]["total"] == 1
    # A group with no trace has no row.
    assert "model-b|other-task" not in groups


@pytest.mark.spec("MEDULLA-ORCH-VOCAB-R010.2")
def test_cost_per_solved_task_abstains_without_solved_trace(clean_graph):
    _ingest(clean_graph)
    result = clean_graph.query.uql("harness_cost_per_solved_task")
    groups = _by_group(result)
    # model-a / shell-task / config-1 solved one trace; both of its traces cost.
    assert groups["model-a|shell-task|config-1"][
        "cost_per_solved_task"
    ] == pytest.approx(0.352)
    # No solved trace: abstained with the reason, never a zero cost.
    assert groups["model-b|shell-task|config-2"]["cost_per_solved_task"] is None
    assert any("NO_SOLVED_TRACE" in warning for warning in result["warnings"])


@pytest.mark.spec("MEDULLA-ORCH-VOCAB-R010.3")
def test_overflow_rate_counts_only_context_overflow(clean_graph):
    _ingest(clean_graph)
    groups = _by_group(clean_graph.query.uql("harness_overflow_rate"))
    assert groups["model-a|shell-task"]["overflow_rate"] == pytest.approx(0.5)
    assert groups["model-a|edit-task"]["overflow"] == 1
    # stuck, step_budget and error count in the denominator only.
    assert groups["model-b|shell-task"]["overflow_rate"] == pytest.approx(0.0)
    assert groups["model-b|edit-task"]["overflow_rate"] == pytest.approx(0.0)


@pytest.mark.spec("MEDULLA-ORCH-VOCAB-R010.4")
def test_mean_peak_ratio_per_model_and_task_class(clean_graph):
    _ingest(clean_graph)
    groups = _by_group(clean_graph.query.uql("harness_mean_peak_ratio"))
    assert groups["model-a|shell-task"]["mean_peak_ratio"] == pytest.approx(
        (6200 / 8000 + 1.0) / 2
    )
    assert groups["model-b|edit-task"]["mean_peak_ratio"] == pytest.approx(7400 / 8000)
    # A zero budget is excluded and counted, not divided.
    clean_graph.nodes.add(
        "trace-zero-budget",
        {
            "node_type": "RunTrace",
            "status": "failed",
            "terminationReason": "error",
            "peakContextTokens": 0,
            "contextBudgetTokens": 0,
            "runModel": "model-z",
            "runTaskClass": "shell-task",
        },
    )
    zero = _by_group(clean_graph.query.uql("harness_mean_peak_ratio"))
    assert zero["model-z|shell-task"]["excluded"] == 1
    assert zero["model-z|shell-task"]["mean_peak_ratio"] is None


@pytest.mark.spec("MEDULLA-ORCH-VOCAB-R010.5")
def test_mechanism_counts_per_config(clean_graph):
    _ingest(clean_graph)
    groups = _by_group(clean_graph.query.uql("harness_mechanism_counts"))
    # model-b / config-2: stuck (1, 0, 20, 19) and step_budget (2, 1, 40, 38).
    assert groups["model-b|config-2"] == {
        "mean_elision_count": pytest.approx(1.5),
        "mean_summarization_count": pytest.approx(0.5),
        "mean_turn_count": pytest.approx(30.0),
        "mean_tool_call_count": pytest.approx(28.5),
    }
    # A missing counter is excluded from its own mean only.
    clean_graph.nodes.add(
        "trace-partial",
        {
            "node_type": "RunTrace",
            "status": "failed",
            "terminationReason": "error",
            "turnCount": 4,
            "runModel": "model-p",
            "appliedHarnessConfig": "config-p",
        },
    )
    partial = _by_group(clean_graph.query.uql("harness_mechanism_counts"))
    assert partial["model-p|config-p"]["mean_turn_count"] == pytest.approx(4.0)
    assert partial["model-p|config-p"]["mean_elision_count"] is None
