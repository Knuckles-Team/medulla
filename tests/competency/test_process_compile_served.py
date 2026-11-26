"""MEDULLA-CQ-ORCH-R700.6 served process compile.

L4: every call goes through the running engine over the wire client. A process
of 12 nodes is written to the caller's graph, compiled, and read back as a
stored workflow version; a cycle of hand-offs with no loop gate is refused.
"""

from __future__ import annotations

import asyncio
import os
import uuid
from typing import Any

import pytest
from conftest import request_context

from medulla.client import MedullaClient

GRAPH = "__commons__"


def _node(node_id: str, element: str) -> dict[str, Any]:
    return {
        "op": "upsert_node",
        "id": node_id,
        "properties": {"type": element, "name": node_id},
    }


def _edge(source: str, target: str, relationship: str) -> dict[str, Any]:
    return {
        "op": "upsert_edge",
        "source": source,
        "target": target,
        "properties": {"relationship": relationship},
    }


def _process(prefix: str) -> tuple[str, list[dict[str, Any]]]:
    """A process of 12 nodes: a start, ten user tasks and an end."""
    process = f"{prefix}:process"
    ids = [f"{prefix}:s{i:02d}" for i in range(12)]
    operations = [_node(process, "Process")]
    for index, node_id in enumerate(ids):
        element = (
            "startEvent" if index == 0 else "endEvent" if index == 11 else "userTask"
        )
        operations.append(_node(node_id, element))
    operations.append(_edge(process, ids[0], "hasStep"))
    for source, target in zip(ids, ids[1:], strict=False):
        operations.append(_edge(source, target, "handsOffTo"))
    return process, operations


async def _connect() -> MedullaClient:
    return await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
        graph_name=GRAPH,
    )


async def _compile_round_trip(prefix: str) -> tuple[dict[str, Any], dict[str, Any]]:
    client = await _connect()
    try:
        process, operations = _process(prefix)
        await client.lifecycle.batch_update(operations)
        first = await client.graph.process_compile(process, 0)
        second = await client.graph.process_compile(process, first["version"])
        return first, second
    finally:
        await client.close()


async def _compile_cycle(prefix: str) -> str:
    client = await _connect()
    try:
        process = f"{prefix}:loop"
        await client.lifecycle.batch_update(
            [
                _node(process, "Process"),
                _node(f"{prefix}:a", "userTask"),
                _node(f"{prefix}:b", "userTask"),
                _edge(process, f"{prefix}:a", "hasStep"),
                _edge(f"{prefix}:a", f"{prefix}:b", "handsOffTo"),
                _edge(f"{prefix}:b", f"{prefix}:a", "handsOffTo"),
            ]
        )
        try:
            await client.graph.process_compile(process)
        except Exception as error:  # the refusal is the answer under test
            return str(error)
        return ""
    finally:
        await client.close()


@pytest.mark.spec("MEDULLA-CQ-ORCH-R700.6")
def test_compile_round_trip(clean_graph):
    # spec: MEDULLA-CQ-ORCH-R700.6
    prefix = f"pc{uuid.uuid4().hex[:8]}"
    first, second = asyncio.run(_compile_round_trip(prefix))
    assert first["workflow_id"] == f"workflow:{prefix}:process:v1"
    assert first["version"] == 1
    assert len(first["workflow_digest"]) == 64
    assert len(first["process_revision"]) == 64
    assert second["version"] == 2
    assert second["workflow_digest"] == first["workflow_digest"]


@pytest.mark.spec("MEDULLA-CQ-ORCH-R700.6")
def test_cycle_without_gate_refused(clean_graph):
    # spec: MEDULLA-CQ-ORCH-R700.6
    prefix = f"pc{uuid.uuid4().hex[:8]}"
    assert "PROCESS_CYCLE_WITHOUT_GATE" in asyncio.run(_compile_cycle(prefix))
