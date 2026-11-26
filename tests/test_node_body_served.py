"""MEDULLA-INGEST-WORKER-R003.21: the served ``NodeBody`` read."""

from __future__ import annotations

import asyncio
import os
import uuid

import msgpack
import pytest
from conftest import TEST_TENANT, request_context

from medulla.client import GraphOperationsClient, MedullaClient


async def _scenario(graph: str) -> None:
    client = await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
        timeout=30,
    )
    try:
        await client.tenants.create(graph, "Global")
        from medulla.generated import graph as generated_graph

        for node_id, props in (
            ("doc:1", {"node_type": "Doc", "revision": 2, "content": "the body"}),
            ("doc:bare", {"node_type": "Doc", "revision": 1}),
        ):
            await generated_graph.send_add_node(
                client,
                {"node_id": node_id, "properties_msgpack": msgpack.packb(props)},
                graph,
            )
        reader = GraphOperationsClient(client)
        answer = await reader.node_body(TEST_TENANT, "doc:1", revision=2, graph=graph)
        assert answer["body"] == "the body"
        assert answer["revision"] == 2
        assert answer["content_digest"]
        current = await reader.node_body(TEST_TENANT, "doc:1", graph=graph)
        assert current["content_digest"] == answer["content_digest"]
        with pytest.raises(Exception, match="NODE_BODY_STALE"):
            await reader.node_body(TEST_TENANT, "doc:1", revision=1, graph=graph)
        # A bodiless node and an absent one refuse alike.
        for node_id in ("doc:bare", "doc:absent"):
            with pytest.raises(Exception, match="NODE_BODY_UNAVAILABLE"):
                await reader.node_body(TEST_TENANT, node_id, graph=graph)
        with pytest.raises(Exception, match="ACCESS_DENIED"):
            await reader.node_body("other-tenant", "doc:1", graph=graph)
        await client.tenants.delete(graph)
    finally:
        await client.close()


# spec: MEDULLA-INGEST-WORKER-R003.21
@pytest.mark.spec("MEDULLA-INGEST-WORKER-R003.21")
def test_a_node_body_is_read_at_its_revision_and_refused_alike_when_absent():
    asyncio.run(_scenario(f"node-body-{uuid.uuid4().hex[:8]}"))
