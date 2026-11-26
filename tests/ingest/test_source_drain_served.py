"""Served chunked source drains (MEDULLA-INGEST-WORKER-R605.2 to R605.5).

`StartSourceDrain` answers a handle and enqueues page 0. A page ack that carries a
cursor enqueues the next page; one without a cursor closes the drain. A redelivered
page is dropped with `DRAIN_CURSOR_STALE`, and `SourceDrainStatus` reports the sums.
"""

from __future__ import annotations

import os
import uuid
from typing import Any

import pytest
from conftest import TEST_TENANT, request_context

from medulla.client import MedullaClient

from .test_source_stream_versions_served import (
    STREAM,
    _batch,
    _close,
    _connector,
    _ingest_client,
    _submit,
)

pytestmark = [
    pytest.mark.spec("MEDULLA-INGEST-WORKER-R605.2"),
    pytest.mark.spec("MEDULLA-INGEST-WORKER-R605.4"),
    pytest.mark.spec("MEDULLA-INGEST-WORKER-R605.3"),
    pytest.mark.spec("MEDULLA-INGEST-WORKER-R605.5"),
]

WORKER = "drain-worker-1"


async def _client() -> MedullaClient:
    return await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
    )


async def _register(client: MedullaClient) -> None:
    await client._send(
        "WorkerRegister",
        {
            "registration": {
                "worker_id": WORKER,
                "host_iri": "host:alpha",
                "cores": 8,
                "memory_bytes": 8 * 1024**3,
                "lanes_supported": ["general"],
            }
        },
    )
    await client._send("WorkerHeartbeat", {"worker_id": WORKER})


async def _claim_page(
    client: MedullaClient, drain_id: str, page: int
) -> dict[str, Any]:
    """Claim tasks, acking every other one, until page `page` of `drain_id` arrives."""
    while True:
        claimed = await client._send(
            "TaskQueueClaim",
            {"worker_id": WORKER, "grant_id": "grant-1", "lease_ms": 60_000},
        )
        assert claimed is not None, f"page {page} of {drain_id} was not enqueued"
        if (
            claimed["task_id"]
            and claimed["payload_ref"].find(f"drain:{drain_id}:{page}") >= 0
        ):
            return claimed
        await client._send(
            "TaskQueueAck",
            {
                "task_id": claimed["task_id"],
                "worker_id": WORKER,
                "outcome": "Succeeded",
                "detail": "",
            },
        )


def _ack(
    task_id: str, items: int, cursor: str | None, wall_ms: int = 3
) -> dict[str, Any]:
    return {
        "task_id": task_id,
        "worker_id": WORKER,
        "outcome": "Succeeded",
        "detail": "",
        "drain": {
            "items": items,
            "next_cursor": cursor,
            "profile": [
                {"stage": "fetch", "wall_ms": wall_ms, "tokens": 0, "embeddings": 0}
            ],
        },
    }


# spec: MEDULLA-INGEST-WORKER-R605.5
@pytest.mark.asyncio
async def test_drain_chains_pages_and_reports() -> None:
    client = await _client()
    try:
        await _register(client)
        stream = f"accounts-{uuid.uuid4().hex[:8]}"
        handle = await client._send(
            "StartSourceDrain", {"connector": "crm", "stream": stream, "mode": "full"}
        )
        drain_id = handle["drain_id"]
        assert len(drain_id) == 64
        for page, (items, cursor) in enumerate([(10, "c1"), (20, "c2"), (5, None)]):
            claimed = await _claim_page(client, drain_id, page)
            await client._send("TaskQueueAck", _ack(claimed["task_id"], items, cursor))
        status = await client._send("SourceDrainStatus", {"drain_id": drain_id})
        assert status["pages"] == 3
        assert status["items"] == 35
        assert status["cursor"] == "c2"
        assert status["state"] == "complete"
        assert status["profile"] == [
            {"stage": "fetch", "wall_ms": 9, "tokens": 0, "embeddings": 0}
        ]
    finally:
        await client.close()


# spec: MEDULLA-INGEST-WORKER-R605.4
@pytest.mark.asyncio
async def test_redelivered_page_is_dropped() -> None:
    client = await _client()
    try:
        await _register(client)
        stream = f"orders-{uuid.uuid4().hex[:8]}"
        handle = await client._send(
            "StartSourceDrain", {"connector": "crm", "stream": stream, "mode": "delta"}
        )
        drain_id = handle["drain_id"]
        page0 = await _claim_page(client, drain_id, 0)
        await client._send("TaskQueueAck", _ack(page0["task_id"], 7, "c1"))
        page1 = await _claim_page(client, drain_id, 1)
        await client._send("TaskQueueAck", _ack(page1["task_id"], 4, "c2"))
        # Page 1 acked again: below the stored checkpoint, so it is dropped.
        with pytest.raises(Exception, match="DRAIN_CURSOR_STALE"):
            await client._send("TaskQueueAck", _ack(page1["task_id"], 4, "c2"))
        status = await client._send("SourceDrainStatus", {"drain_id": drain_id})
        assert (status["pages"], status["items"], status["cursor"]) == (2, 11, "c2")
        assert status["state"] == "running"
        # An id no drain of this tenant has reads as unknown.
        with pytest.raises(Exception, match="DRAIN_UNKNOWN"):
            await client._send("SourceDrainStatus", {"drain_id": "0" * 64})
    finally:
        await client.close()


# spec: MEDULLA-INGEST-WORKER-R605.3
@pytest.mark.asyncio
async def test_ingest_lands_in_the_routed_graph() -> None:
    connector = _connector()
    sent_to, sent_graph = await _ingest_client(connector)
    routed_to, routed_graph = await _ingest_client(connector)
    control = await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
        graph_name="main",
    )
    try:
        # The connector's routing record names the second graph.
        await control.lifecycle.batch_update(
            [
                {
                    "op": "upsert_node",
                    "id": f"ingest_route:source:{connector}",
                    "properties": {
                        "type": "IngestRoute",
                        "graph": routed_graph,
                        "tenant_id": TEST_TENANT,
                    },
                }
            ]
        )
        await _submit(sent_to, _batch(connector, 1, None, "routed"), "routed")
        landed = await routed_to.query.uql(f"MATCH () WHERE source_stream = '{STREAM}'")
        assert len(landed["rows"]) == 1, landed
        left = await sent_to.query.uql(f"MATCH () WHERE source_stream = '{STREAM}'")
        assert left["rows"] == [], left
    finally:
        await control.lifecycle.batch_update(
            [{"op": "remove_node", "id": f"ingest_route:source:{connector}"}]
        )
        await control.close()
        await _close(sent_to, sent_graph)
        await _close(routed_to, routed_graph)
