"""MEDULLA-SDLC-R014.6: the competency questions over the spec artifact graph.

L4: every call goes through the running engine. Each test seeds the records and edges
of one question with a batch, then asks the served ``SpecQuestion`` and reads the rows.
"""

from __future__ import annotations

import asyncio
import os
import uuid

import msgpack
import pytest
from conftest import request_context

from medulla.client import MedullaClient


def _body(payload):
    if isinstance(payload, (bytes, bytearray)):
        return msgpack.unpackb(payload, raw=False)
    return payload


def _node(node_id: str, node_type: str, **props: str) -> dict:
    return {
        "op": "upsert_node",
        "id": node_id,
        "properties": {"type": node_type, **props},
    }


def _edge(source: str, relationship: str, target: str) -> dict:
    return {
        "op": "upsert_edge",
        "source": source,
        "target": target,
        "properties": {"relationship": relationship},
    }


async def _ask(seed: list[dict], question: str, subject: str | None = None):
    graph = f"sdlc-spec-q-{uuid.uuid4().hex[:8]}"
    client = await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
        timeout=30,
    )
    try:
        await client.tenants.create(graph, "Global")
        await client._send(
            "BatchUpdate",
            {"operations_msgpack": msgpack.packb(seed, use_bin_type=True)},
            graph,
        )
        try:
            return _body(await client.spec_question(question, graph, subject))
        finally:
            await client.tenants.delete(graph)
    finally:
        await client.close()


def _run(seed: list[dict], question: str, subject: str | None = None):
    return asyncio.run(_ask(seed, question, subject))


def _requirement(row: str, status: str = "SPECIFIED") -> dict:
    return _node(f"requirement:demo:{row}", "Requirement", rowId=row, rowStatus=status)


# spec: MEDULLA-SDLC-R014.6
@pytest.mark.spec("MEDULLA-SDLC-R014.6")
def test_unreachable_requirements():
    seed = [
        _requirement("D-R1"),
        _requirement("D-R2"),
        _node("testfile:demo:t.py", "TestFile"),
        _edge("requirement:demo:D-R2", "verifiedBy", "testfile:demo:t.py"),
    ]
    answer = _run(seed, "unreachable_requirements")
    assert [row["requirement"] for row in answer["rows"]] == ["requirement:demo:D-R1"]
    assert answer["digests"]


# spec: MEDULLA-SDLC-R014.6
@pytest.mark.spec("MEDULLA-SDLC-R014.6")
def test_uncovered_code_items():
    seed = [
        _requirement("D-R1"),
        _node("symbol:a", "SYMBOL"),
        _node("symbol:b", "SYMBOL"),
        _edge("requirement:demo:D-R1", "implementedBy", "symbol:a"),
    ]
    answer = _run(seed, "uncovered_code_items")
    assert [row["symbol"] for row in answer["rows"]] == ["symbol:b"]


# spec: MEDULLA-SDLC-R014.6
@pytest.mark.spec("MEDULLA-SDLC-R014.6")
def test_tasks_blocked_by_contract():
    seed = [
        _requirement("D-R1"),
        _requirement("D-R2", "VERIFIED"),
        _requirement("D-P1"),
        _node("contract:demo:c", "Contract", provider_rows="demo:D-P1"),
        _edge("requirement:demo:D-R1", "crossesSeam", "contract:demo:c"),
        _node("task:demo:t#T001", "TaskLine", taskId="T001", phase="1", ticked="false"),
        _node("task:demo:t#T002", "TaskLine", taskId="T002", phase="1", ticked="false"),
        _edge("task:demo:t#T001", "forRow", "requirement:demo:D-R1"),
        _edge("task:demo:t#T002", "forRow", "requirement:demo:D-R2"),
    ]
    answer = _run(seed, "tasks_blocked_by_contract")
    assert [row["task"] for row in answer["rows"]] == ["task:demo:t#T001"]
    assert answer["rows"][0]["provider"] == "requirement:demo:D-P1"


# spec: MEDULLA-SDLC-R014.6
@pytest.mark.spec("MEDULLA-SDLC-R014.6")
def test_principles_a_change_breaks():
    seed = [
        _requirement("D-R1"),
        _node(
            "principle:demo:XI", "ConstitutionPrinciple", key="XI", title="Article XI"
        ),
        _node("commit:abc", "Changeset"),
        _edge("commit:abc", "citesRequirement", "requirement:demo:D-R1"),
        _edge("requirement:demo:D-R1", "governedBy", "principle:demo:XI"),
    ]
    answer = _run(seed, "principles_at_risk", "abc")
    assert [row["key"] for row in answer["rows"]] == ["XI"]
    assert _run(seed, "principles_at_risk", "nothing-cited")["rows"] == []


# spec: MEDULLA-SDLC-R014.6
@pytest.mark.spec("MEDULLA-SDLC-R014.6")
def test_changes_advancing_goal():
    seed = [
        _requirement("D-R1"),
        _node("goal:demo:g", "Goal", project="demo", statement="ship"),
        _edge("requirement:demo:D-R1", "targetsGoal", "goal:demo:g"),
        _node("commit:abc", "Changeset"),
        _edge("commit:abc", "citesRequirement", "requirement:demo:D-R1"),
    ]
    answer = _run(seed, "changes_advancing_goal", "goal:demo:g")
    assert [row["changeset"] for row in answer["rows"]] == ["commit:abc"]
    with pytest.raises(Exception, match="COMPETENCY_SUBJECT_UNKNOWN"):
        _run(seed, "changes_advancing_goal", "goal:demo:missing")
    with pytest.raises(Exception, match="PARAMETER_INVALID"):
        _run(seed, "changes_advancing_goal")


# spec: MEDULLA-SDLC-R014.6
@pytest.mark.spec("MEDULLA-SDLC-R014.6")
def test_next_unblocked_task_for_phase():
    seed = [
        _requirement("D-R1", "VERIFIED"),
        _requirement("D-R2"),
        _node("task:demo:t#T001", "TaskLine", taskId="T001", phase="1", ticked="true"),
        _node("task:demo:t#T002", "TaskLine", taskId="T002", phase="1", ticked="false"),
        _node("task:demo:t#T010", "TaskLine", taskId="T010", phase="1", ticked="false"),
        _edge("task:demo:t#T001", "forRow", "requirement:demo:D-R1"),
        _edge("task:demo:t#T002", "forRow", "requirement:demo:D-R2"),
        _edge("task:demo:t#T010", "forRow", "requirement:demo:D-R1"),
    ]
    answer = _run(seed, "next_unblocked_task", "1")
    assert [row["task_id"] for row in answer["rows"]] == ["T002"]
    with pytest.raises(Exception, match="COMPETENCY_SUBJECT_UNKNOWN"):
        _run(seed, "next_unblocked_task", "9")
