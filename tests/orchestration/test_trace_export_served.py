"""MEDULLA-ORCH-VOCAB-R009.100 served correlated run-trace export.

A parent run with one child agent run, two spans and one tool call is written through
the served graph writes; ``run_trace_export`` pages it. Every call goes through the
running engine over the wire client.
"""

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
PARENT = "run:export-parent"
CHILD = "run:export-child"


def _trace(declared: list[str]) -> dict:
    """A shape-valid run trace that declares the spans it produced."""
    base = json.loads(FIXTURES.read_text())["traces"][0]
    props = {key: value for key, value in base.items() if key != "id"}
    props["node_type"] = "RunTrace"
    props["declaredSpans"] = json.dumps(declared)
    return props


def _seed(graph, declared: list[str]) -> None:
    graph.nodes.add(PARENT, _trace(declared))
    child = _trace([])
    child.pop("declaredSpans")
    graph.nodes.add(CHILD, child)
    graph.nodes.add("span:export-a", {"node_type": "Span", "started_at_ms": 110})
    graph.nodes.add("span:export-b", {"node_type": "Span", "started_at_ms": 120})
    graph.nodes.add("call:export-1", {"node_type": "ToolCall", "tool_name": "t"})
    graph.edges.add(PARENT, CHILD, {"relationship": "hasChildRun"})
    graph.edges.add(PARENT, "span:export-a", {"relationship": "hasSpan"})
    graph.edges.add(PARENT, "span:export-b", {"relationship": "hasSpan"})
    graph.edges.add(CHILD, "call:export-1", {"relationship": "hasToolCall"})


@pytest.mark.spec("MEDULLA-ORCH-VOCAB-R009.100")
def test_parent_with_children_exports_complete_stable_digest(clean_graph):
    # spec: MEDULLA-ORCH-VOCAB-R009.100
    _seed(clean_graph, ["span:export-a", "span:export-b"])
    first = clean_graph.graph.run_trace_export(PARENT)
    assert first["complete"] is True and first.get("refusal") is None, first
    kinds = {record["record_ref"]: record["kind"] for record in first["records"]}
    assert kinds == {
        PARENT: "parent_run",
        CHILD: "child_agent",
        "span:export-a": "span",
        "span:export-b": "span",
        "call:export-1": "tool_call",
    }
    assert all("properties" not in record for record in first["records"])
    # The digest is stable across reads and across page sizes.
    again = clean_graph.graph.run_trace_export(PARENT, page_size=2)
    assert again["snapshot_digest"] == first["snapshot_digest"]
    assert len(again["records"]) == 2 and again["next_page_token"]
    rest = clean_graph.graph.run_trace_export(
        PARENT, page_size=10, page_token=again["next_page_token"]
    )
    refs = [r["record_ref"] for r in again["records"] + rest["records"]]
    assert refs == [r["record_ref"] for r in first["records"]]


@pytest.mark.spec("MEDULLA-ORCH-VOCAB-R009.100")
def test_declared_span_that_is_missing_is_never_a_silent_gap(clean_graph):
    # spec: MEDULLA-ORCH-VOCAB-R009.100
    _seed(clean_graph, ["span:export-a", "span:export-b", "span:export-lost"])
    page = clean_graph.graph.run_trace_export(PARENT)
    assert page["complete"] is False
    assert page["refusal"] == "EXPORT_CHILD_MISSING"
    assert page["missing_children"] == ["span:export-lost"]


@pytest.mark.spec("MEDULLA-ORCH-VOCAB-R009.100")
def test_unknown_run_reads_as_absent_and_a_stale_token_refuses(clean_graph):
    # spec: MEDULLA-ORCH-VOCAB-R009.100
    _seed(clean_graph, ["span:export-a", "span:export-b"])
    with pytest.raises(Exception, match="RUN_TRACE_NOT_FOUND"):
        clean_graph.graph.run_trace_export("run:nobody")
    with pytest.raises(Exception, match="EXPORT_CURSOR_STALE"):
        clean_graph.graph.run_trace_export(PARENT, page_token="sha256:00:0")
    stripped = clean_graph.graph.run_trace_export(
        PARENT, redaction_profile="strip_content"
    )
    assert all("properties" in record for record in stripped["records"])
