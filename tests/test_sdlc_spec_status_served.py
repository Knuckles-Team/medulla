"""MEDULLA-SDLC-R005.3 and R005.4: the served ``SpecStatus`` read.

L4: every call goes through the running engine. Each test seeds the requirement rows,
changeset nodes and test bindings of one repository with a batch, then reads the status.
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


def _seed() -> list[dict]:
    return [
        _node(
            "requirement:demo:DEMO-A-R001",
            "Requirement",
            repo="demo",
            rowId="DEMO-A-R001",
        ),
        _node(
            "requirement:demo:DEMO-A-R002",
            "Requirement",
            repo="demo",
            rowId="DEMO-A-R002",
        ),
        _node(
            "changeset:demo:c1",
            "Changeset",
            repo="demo",
            changeset_id="c1",
            created_at="1",
        ),
        _node(
            "changeset:demo:c2",
            "Changeset",
            repo="demo",
            changeset_id="c2",
            created_at="2",
        ),
        _edge("changeset:demo:c1", "citesRequirement", "requirement:demo:DEMO-A-R001"),
        _edge("changeset:demo:c2", "citesRequirement", "requirement:demo:DEMO-A-R002"),
        _node(
            "testfile:demo:tests/test_a.py",
            "TestFile",
            repo="demo",
            path="tests/test_a.py",
        ),
        _edge(
            "requirement:demo:DEMO-A-R001", "boundTest", "testfile:demo:tests/test_a.py"
        ),
    ]


async def _with_graph(seed: list[dict], body):
    graph = f"sdlc-status-{uuid.uuid4().hex[:8]}"
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
            return await body(client, graph)
        finally:
            await client.tenants.delete(graph)
    finally:
        await client.close()


# spec: MEDULLA-SDLC-R005.3
@pytest.mark.spec("MEDULLA-SDLC-R005.3")
def test_served_status_reads_graph():
    async def body(client, graph):
        return _body(await client.spec_status("demo", "c2", graph))

    answer = asyncio.run(_with_graph(_seed(), body))
    states = {row["id"]: row["state"] for row in answer["rows"]}
    assert states == {"DEMO-A-R001": "VERIFIED", "DEMO-A-R002": "LANDED"}
    first = next(row for row in answer["rows"] if row["id"] == "DEMO-A-R001")
    assert first["landed_by"] == ["c1"] and first["verified_by"] == ["tests/test_a.py"]


# spec: MEDULLA-SDLC-R005.3
@pytest.mark.spec("MEDULLA-SDLC-R005.3")
def test_status_at_an_earlier_rev_and_spec_filter():
    async def body(client, graph):
        early = _body(await client.spec_status("demo", "c1", graph, "DEMO-A"))
        none = _body(await client.spec_status("demo", "c1", graph, "NOPE"))
        return early, none

    early, none = asyncio.run(_with_graph(_seed(), body))
    states = {row["id"]: row["state"] for row in early["rows"]}
    assert states["DEMO-A-R002"] == "SPECIFIED"
    assert none["rows"] == []


# spec: MEDULLA-SDLC-R005.4
@pytest.mark.spec("MEDULLA-SDLC-R005.4")
def test_status_refusals():
    async def body(client, graph):
        with pytest.raises(Exception, match="SPEC_STATUS_REV_UNKNOWN"):
            await client.spec_status("demo", "nope", graph)
        with pytest.raises(Exception, match="SPEC_TREE_ABSENT"):
            await client.spec_status("elsewhere", "c1", graph)

    asyncio.run(_with_graph(_seed(), body))
    # A graph that holds no spec tree is absent too.
    asyncio.run(_with_graph([], _absent))


async def _absent(client, graph):
    with pytest.raises(Exception, match="SPEC_TREE_ABSENT"):
        await client.spec_status("demo", "c1", graph)


# spec: MEDULLA-SDLC-R005.5
@pytest.mark.spec("MEDULLA-SDLC-R005.5")
def test_matches_shared_generator():
    """Each row equals the shared status generator on the same revision: LANDED when a
    changeset cites it, VERIFIED when it is also bound to a test, and a landed row names
    its changeset ids in ``landed_by``."""

    def expected(cited_by, bound_tests):
        if not cited_by:
            return "SPECIFIED"
        return "VERIFIED" if bound_tests else "LANDED"

    async def body(client, graph):
        return _body(await client.spec_status("demo", "c2", graph))

    answer = asyncio.run(_with_graph(_seed(), body))
    cited = {"DEMO-A-R001": ["c1"], "DEMO-A-R002": ["c2"]}
    bound = {"DEMO-A-R001": ["tests/test_a.py"], "DEMO-A-R002": []}
    for row in answer["rows"]:
        assert row["state"] == expected(cited[row["id"]], bound[row["id"]])
        assert row["landed_by"] == cited[row["id"]]
        assert row["verified_by"] == bound[row["id"]]
