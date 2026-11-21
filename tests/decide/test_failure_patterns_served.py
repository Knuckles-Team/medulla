"""MEDULLA-DECISION-ENGINE-R701.3 served failure-pattern ingest.

L4: every call goes through the running engine over the wire client. Three
failed ``RunTrace`` nodes of one signature are written to the caller's graph
through ``BatchUpdate``; ``FailureIngest`` ingest files one gap for the
pattern, and a repeat ingest files none.
"""

from __future__ import annotations

import asyncio
import os
import uuid
from typing import Any

import pytest
from conftest import request_context

from medulla import decision_client as dc
from medulla.client import MedullaClient

GRAPH = "__commons__"


def _run_operations(prefix: str, index: int) -> list[dict[str, Any]]:
    run = f"{prefix}-run-{index}"
    plan, call = f"{run}-plan", f"{run}-call"
    return [
        {
            "op": "upsert_node",
            "id": run,
            "properties": {
                "node_type": "RunTrace",
                "run_trace_id": f"{prefix}-trace-{index}",
                "terminationReason": f"{prefix}_max_steps",
                "endMs": 1_700_000_000_000 + index,
            },
        },
        {"op": "upsert_node", "id": plan, "properties": {"kind": "plan"}},
        {"op": "upsert_node", "id": call, "properties": {"kind": "tool_call"}},
        {"op": "upsert_node", "id": f"{prefix}-tool", "properties": {"kind": "tool"}},
        {
            "op": "upsert_edge",
            "source": run,
            "target": plan,
            "properties": {"relationship": "hasStep"},
        },
        {
            "op": "upsert_edge",
            "source": run,
            "target": call,
            "properties": {"relationship": "hasStep"},
        },
        {
            "op": "upsert_edge",
            "source": plan,
            "target": call,
            "properties": {"relationship": "precedes"},
        },
        {
            "op": "upsert_edge",
            "source": call,
            "target": f"{prefix}-tool",
            "properties": {"relationship": "usesTool"},
        },
    ]


async def _seed_and_ingest(prefix: str) -> tuple[dict[str, Any], dict[str, Any]]:
    client = await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
        graph_name=GRAPH,
    )
    try:
        for index in range(3):
            await client.batch_update(_run_operations(prefix, index))
        decisions = dc.DecisionClient(client=client, graph=GRAPH)
        first = await decisions.failure_ingest(dc.failure_ingest_op(GRAPH))
        second = await decisions.failure_ingest(dc.failure_ingest_op(GRAPH))
        return first, second
    finally:
        await client.close()


@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R701.3")
def test_ingest_served(clean_graph):
    # spec: MEDULLA-DECISION-ENGINE-R701.3
    # Letters outside a-f only, so signature normalisation keeps the class text.
    prefix = "fp" + "".join(chr(103 + int(c, 16)) for c in uuid.uuid4().hex[:8])
    first, second = asyncio.run(_seed_and_ingest(prefix))

    (pattern,) = [
        p for p in first["patterns"] if p["signature"].startswith(f"{prefix}_max_steps")
    ]
    assert pattern["count"] == 3
    assert pattern["signature"].endswith("|tool_call")
    assert len(first["filed"]) >= 1

    # A repeat ingest files nothing: the gap node already exists.
    assert second["filed"] == []
