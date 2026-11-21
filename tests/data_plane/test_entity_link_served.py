"""Only `ApproveEntityLink` writes an `owl:sameAs` edge (MEDULLA-UNIFIED-DATA-PLANE-R030.4).

A raw `AddEdge` that names the relation `owl:sameAs` is refused `LINK_NOT_APPROVABLE`
and writes no edge. The approved path still writes the edge for a matching pair.
"""

from __future__ import annotations

import os
import uuid

import pytest
from conftest import request_context

from medulla.client import MedullaClient
from medulla.generated import query

HOST = "grafana.example.com"


async def _connect() -> MedullaClient:
    return await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
    )


@pytest.mark.spec("MEDULLA-UNIFIED-DATA-PLANE-R030.4")
@pytest.mark.asyncio
async def test_raw_add_edge_same_as_is_refused_and_approval_still_writes():
    tag = uuid.uuid4().hex[:8]
    client = await _connect()
    try:
        left = {"application": "uptime", "entity_id": f"mon-{tag}"}
        right = {"application": "container_manager", "entity_id": f"wl-{tag}"}
        left_id = f"uptime:mon-{tag}"
        right_id = f"container_manager:wl-{tag}"
        for node_id in (left_id, right_id):
            await client.nodes.add(
                node_id, {"type": "Service", "name": "Grafana", "host": HOST}
            )

        # A raw AddEdge naming owl:sameAs is refused, and no edge exists afterwards.
        with pytest.raises(Exception, match="LINK_NOT_APPROVABLE"):
            await client.edges.add(left_id, right_id, {"relationship": "owl:sameAs"})
        assert not await client.edges.has(left_id, right_id)

        # An edge of another relationship still passes the generic writer.
        await client.edges.add(left_id, right_id, {"relationship": "monitors"})
        await client.edges.remove(left_id, right_id)

        # The approved path writes the owl:sameAs edge for a matching pair.
        approval = (
            await query.send_approve_entity_link(
                client, {"left": left, "right": right}
            )
        ).payload
        assert approval["predicate"] == "owl:sameAs"
        assert await client.edges.has(left_id, right_id)
    finally:
        await client.close()
