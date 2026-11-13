"""MEDULLA-DURABLE-KERNEL-R871: the served ``Batch`` method.

One transaction of mixed node, memory and claim operations: the writes and the
read commit together, an oversize batch refuses ``BATCH_TOO_LARGE`` and writes
nothing, an empty batch refuses ``INVALID_ARGUMENT``, and a batch holding one
operation the caller may not run writes nothing.
"""

from __future__ import annotations

import asyncio
import os
import uuid
from typing import Any

import pytest
from conftest import request_context

from medulla.client import MedullaClient

# The default of the registry knob `batch.max_operations`.
MAX_BATCH_OPERATIONS = 64


async def _client(scopes: list[str] | None = None) -> MedullaClient:
    context = request_context() if scopes is None else request_context(scopes=scopes)
    return await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=context,
    )


def _create(node_id: str, **properties: Any) -> dict[str, Any]:
    return {"op": "CreateNodeIfAbsent", "node_id": node_id, "properties": properties}


async def _refused(client: MedullaClient, operations: list[dict[str, Any]]) -> str:
    with pytest.raises(Exception) as raised:
        await client.batch(operations)
    return str(raised.value)


async def _commits_once_and_refuses_oversize() -> None:
    tag = uuid.uuid4().hex[:8]
    client = await _client()
    try:
        first, second = f"loop-{tag}-a", f"loop-{tag}-b"
        results = await client.batch(
            [
                _create(first, label=f"task-{tag}", status="pending", seq=1),
                _create(second, label=f"task-{tag}", status="pending", seq=2),
                {
                    "op": "CompareAndSetNodeFields",
                    "node_id": first,
                    "conditions": {"seq": 1},
                    "updates": {"seen": True},
                },
                {
                    "op": "Reinforce",
                    "node_id": first,
                    "now_ms": 1_800_000_000_000,
                    "weight": 1.0,
                },
                {"op": "GetNodeProperties", "node_id": first},
            ]
        )
        assert [result["op"] for result in results] == [
            "CreateNodeIfAbsent",
            "CreateNodeIfAbsent",
            "CompareAndSetNodeFields",
            "Reinforce",
            "GetNodeProperties",
        ]
        assert results[0]["created"] and results[1]["created"]
        assert results[2]["applied"] and results[3]["existed"]
        assert results[4]["properties"]["seen"] is True

        # The batch committed as one: both nodes read back after it.
        after = await client.batch(
            [
                {"op": "GetNodeProperties", "node_id": first},
                {"op": "GetNodeProperties", "node_id": second},
            ]
        )
        assert all(result["properties"] is not None for result in after)

        # Over the cap: refused before any operation runs, nothing written.
        oversize = [
            _create(f"over-{tag}-{index}", label=f"over-{tag}")
            for index in range(MAX_BATCH_OPERATIONS + 1)
        ]
        assert "BATCH_TOO_LARGE" in await _refused(client, oversize)
        probe = await client.batch(
            [{"op": "GetNodeProperties", "node_id": f"over-{tag}-0"}]
        )
        assert probe[0]["properties"] is None

        # A batch at the cap commits.
        at_cap = await client.batch(oversize[:MAX_BATCH_OPERATIONS])
        assert len(at_cap) == MAX_BATCH_OPERATIONS

        # Empty: invalid argument.
        assert "INVALID_ARGUMENT" in await _refused(client, [])
    finally:
        await client.close()


async def _denied_inner_operation_writes_nothing() -> None:
    tag = uuid.uuid4().hex[:8]
    reader = await _client(scopes=["node:read", "node:write"])
    try:
        node = f"deny-{tag}"
        # Reinforce is a memory write: this caller holds node scopes only.
        message = await _refused(
            reader,
            [
                _create(node, label=f"deny-{tag}"),
                {"op": "Reinforce", "node_id": node, "now_ms": 1, "weight": 1.0},
            ],
        )
        assert "ACCESS_DENIED" in message and "operation 1" in message
        probe = await reader.batch([{"op": "GetNodeProperties", "node_id": node}])
        assert probe[0]["properties"] is None
    finally:
        await reader.close()


def test_batch_commits_once_and_refuses_oversize(clean_graph) -> None:
    asyncio.run(_commits_once_and_refuses_oversize())


def test_batch_with_a_denied_inner_operation_writes_nothing(clean_graph) -> None:
    asyncio.run(_denied_inner_operation_writes_nothing())
