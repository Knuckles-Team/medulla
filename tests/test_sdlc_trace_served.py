"""MEDULLA-SDLC-R006.3 to R006.5: the served ``LifecycleTrace`` and lifecycle questions.

L4: every call goes through the running engine; the records are seeded with one batch.
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
        "properties": {"relationship": relationship, "rule": "r1", "rule_version": "1"},
    }


SEED = [
    _node("requirement:d:R1", "Requirement", repo="d", rowId="R1"),
    _node("requirement:d:R2", "Requirement", repo="d", rowId="R2"),
    _node("testfile:d:t", "TestFile"),
    _node("symbol:s", "SYMBOL"),
    _node("commit:c1", "Changeset"),
    _node("commit:c2", "Changeset"),
    _node("release:d:1", "Release"),
    _node("release:d:2", "Release"),
    _node("deployment:d1", "Deployment"),
    _node("deployment:d2", "Deployment"),
    _node("defect:d/1", "Defect", status="triaged"),
    _edge("requirement:d:R1", "verifiedBy", "testfile:d:t"),
    _edge("symbol:s", "implements", "requirement:d:R1"),
    _edge("commit:c1", "changes", "symbol:s"),
    _edge("release:d:1", "contains", "commit:c1"),
    _edge("release:d:2", "contains", "commit:c1"),
    _edge("release:d:2", "contains", "commit:c2"),
    _edge("deployment:d1", "deploys", "release:d:1"),
    _edge("deployment:d2", "deploys", "release:d:2"),
    _edge("defect:d/1", "affects", "symbol:s"),
]


async def _with_graph(body):
    graph = f"sdlc-trace-{uuid.uuid4().hex[:8]}"
    client = await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
        timeout=30,
    )
    try:
        await client.tenants.create(graph, "Global")
        await client._send(
            "BatchUpdate",
            {"operations_msgpack": msgpack.packb(SEED, use_bin_type=True)},
            graph,
        )
        try:
            return await body(client, graph)
        finally:
            await client.tenants.delete(graph)
    finally:
        await client.close()


# spec: MEDULLA-SDLC-R006.3
@pytest.mark.spec("MEDULLA-SDLC-R006.3")
def test_competency_questions_answer():
    async def body(client, graph):
        async def ask(question, subject=None):
            return _body(await client.spec_question(question, graph, subject))

        return (
            await ask("unbound_requirements"),
            await ask("deployment_of_requirement", "requirement:d:R1"),
            await ask("open_defects_in_release", "release:d:1"),
            await ask("changesets_between_deployments", "deployment:d2,deployment:d1"),
        )

    unbound, deployed, defects, between = asyncio.run(_with_graph(body))
    assert [r["row"] for r in unbound["rows"]] == ["R2"]
    assert {r["deployment"] for r in deployed["rows"]} == {
        "deployment:d1",
        "deployment:d2",
    }
    assert [r["defect"] for r in defects["rows"]] == ["defect:d/1"]
    assert [r["changeset"] for r in between["rows"]] == ["commit:c2"]


# spec: MEDULLA-SDLC-R006.3
@pytest.mark.spec("MEDULLA-SDLC-R006.3")
def test_trace_walks_from_a_requirement():
    async def body(client, graph):
        return _body(
            await client.lifecycle_trace(
                {"anchor": "requirement:d:R1", "depth": 4}, graph
            )
        )

    answer = asyncio.run(_with_graph(body))
    ids = {r["id"] for r in answer["records"]}
    assert {"requirement:d:R1", "symbol:s", "commit:c1", "release:d:1"} <= ids
    assert answer["policy_digest"] and answer["truncated"] is False


# spec: MEDULLA-SDLC-R006.5
@pytest.mark.spec("MEDULLA-SDLC-R006.5")
def test_edges_name_rules():
    async def body(client, graph):
        return _body(
            await client.lifecycle_trace(
                {"anchor": "requirement:d:R1", "depth": 1}, graph
            )
        )

    answer = asyncio.run(_with_graph(body))
    assert answer["edges"], "the verifiedBy edge is reached at depth one"
    assert all(
        edge["rule"] == "r1" and edge["rule_version"] == 1 for edge in answer["edges"]
    )
    assert answer["policy_digest"]


# spec: MEDULLA-SDLC-R006.4
@pytest.mark.spec("MEDULLA-SDLC-R006.4")
def test_trace_refusals():
    async def body(client, graph):
        with pytest.raises(Exception, match="LIFECYCLE_DEPTH_INVALID"):
            await client.lifecycle_trace(
                {"anchor": "requirement:d:R1", "depth": 17}, graph
            )
        with pytest.raises(Exception, match="LIFECYCLE_DEPTH_INVALID"):
            await client.lifecycle_trace(
                {"anchor": "requirement:d:R1", "depth": 0}, graph
            )
        return _body(
            await client.lifecycle_trace(
                {"anchor": "requirement:other:none", "depth": 2}, graph
            )
        )

    absent = asyncio.run(_with_graph(body))
    assert absent["records"] == [] and absent["edges"] == []


# spec: MEDULLA-SDLC-R006.6
@pytest.mark.spec("MEDULLA-SDLC-R006.6")
def test_trace_journey_depth_four():
    """Depth four from a requirement reaches the symbol, the changeset, the release and
    the deployment of the four fixtures of R006.3, and the walk is not truncated; a
    shallower walk reaches fewer records."""

    async def body(client, graph):
        deep = _body(
            await client.lifecycle_trace(
                {"anchor": "requirement:d:R1", "depth": 4}, graph
            )
        )
        shallow = _body(
            await client.lifecycle_trace(
                {"anchor": "requirement:d:R1", "depth": 1}, graph
            )
        )
        return deep, shallow

    deep, shallow = asyncio.run(_with_graph(body))
    reached = {record["id"] for record in deep["records"]}
    assert {
        "requirement:d:R1",
        "symbol:s",
        "commit:c1",
        "release:d:1",
        "deployment:d1",
    } <= reached
    assert deep["truncated"] is False
    assert len(shallow["records"]) < len(deep["records"])
