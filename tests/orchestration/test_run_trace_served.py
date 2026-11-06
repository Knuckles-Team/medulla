"""MEDULLA-ORCH-VOCAB-R009.15 / R009.17 served receipts: a shape-invalid
``RunTrace`` write is refused and stored nowhere, and every run-trace fixture
round-trips its harness properties through the Python client ``add_node``."""

from __future__ import annotations

import json
from pathlib import Path

import pytest

from medulla.client import EngineResponseError

FIXTURES = (
    Path(__file__).resolve().parents[2]
    / "crates"
    / "medulla-core"
    / "tests"
    / "fixtures"
    / "run_traces.json"
)


def _traces() -> list[dict]:
    return json.loads(FIXTURES.read_text())["traces"]


@pytest.mark.spec("MEDULLA-ORCH-VOCAB-R009.15")
def test_invalid_trace_refused_and_not_stored(clean_graph):
    props = {
        "node_type": "RunTrace",
        "terminationReason": "gave_up",
        "turnCount": 3,
    }
    with pytest.raises(EngineResponseError, match="RUN_TRACE_SHAPE_INVALID"):
        clean_graph.nodes.add("trace-invalid", props)
    assert clean_graph.nodes.has("trace-invalid") is False


@pytest.mark.spec("MEDULLA-ORCH-VOCAB-R009.17")
def test_client_ingest_round_trips_harness_properties(clean_graph):
    traces = _traces()
    # One trace per termination reason plus the over-budget overflow, the same
    # fixture set crates/medulla-core/tests/run_trace_ingest.rs counts.
    assert len(traces) == 6
    assert len({trace["terminationReason"] for trace in traces}) == 5
    for trace in traces:
        clean_graph.nodes.add("config-" + trace["id"], {"node_type": "HarnessConfig"})
    for trace in traces:
        props = {key: value for key, value in trace.items() if key != "id"}
        props["node_type"] = "RunTrace"
        clean_graph.nodes.add(trace["id"], props)
        clean_graph.edges.add(
            trace["id"], "config-" + trace["id"], {"label": "appliedHarnessConfig"}
        )
    for trace in traces:
        stored = clean_graph.nodes.properties(trace["id"])
        for key, value in trace.items():
            if key == "id":
                continue
            assert stored[key] == value, (trace["id"], key)
        assert clean_graph.edges.has(trace["id"], "config-" + trace["id"]) is True
