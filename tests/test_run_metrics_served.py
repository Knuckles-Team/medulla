"""MEDULLA-ORCH-VOCAB-R811.1: the served ``RunMetrics`` read.

Cost and tokens are per solved run, a group with no solved run is left out, a
population that solved nothing abstains instead of answering zero, and a tenant
other than the verified one is refused.
"""

from __future__ import annotations

import asyncio
import os
import uuid
from typing import Any

import msgpack
import pytest
from conftest import TEST_TENANT, request_context

from medulla.client import MedullaClient

# (agent, termination, completed_at_ms, cost_usd, output_tokens)
_RUNS = [
    ("a", "final_answer", 10, 1.0, 100),
    ("a", "final_answer", 20, 3.0, 300),
    ("a", "error", 30, 5.0, 500),
    ("b", "final_answer", 40, None, 50),
    ("c", "stuck", 50, 9.0, 900),
]


def _body(payload: Any) -> Any:
    if isinstance(payload, (bytes, bytearray)):
        return msgpack.unpackb(payload, raw=False)
    return payload


async def _metrics(client: MedullaClient, graph: str, request: dict) -> dict:
    return _body(await client._send("RunMetrics", {"request": request}, graph))


async def _scenario(graph: str) -> None:
    client = await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
        timeout=30,
    )
    try:
        await client.tenants.create(graph, "Global")
        from medulla.generated import graph as generated_graph

        for index, (agent, termination, at, run_cost, run_tokens) in enumerate(_RUNS):
            props: dict[str, Any] = {
                "node_type": "RunTrace",
                "run_trace_id": f"trace-{index}",
                "runTaskClass": "fix",
                "agentId": agent,
                "projectId": "p",
                "terminationReason": termination,
                "completedAtMs": at,
                "outputTokens": run_tokens,
            }
            if run_cost is not None:
                props["costUsd"] = run_cost
            await generated_graph.send_add_node(
                client,
                {
                    "node_id": f"run:{index}",
                    "properties_msgpack": msgpack.packb(props),
                },
                graph,
            )

        base = {"tenant": TEST_TENANT, "group_by": "agent"}
        # Agent a solved two runs at 1.0 and 3.0 and its failed run is not charged
        # to a solve; b reports no cost and c solved nothing, so both are left out.
        cost = await _metrics(client, graph, {**base, "query": "cost"})
        assert cost["abstained"] is None
        assert cost["groups"] == [{"key": "a", "value": 2.0, "trace_count": 3}]
        tokens = await _metrics(client, graph, {**base, "query": "tokens"})
        assert {g["key"]: g["value"] for g in tokens["groups"]} == {
            "a": 200.0,
            "b": 50.0,
        }
        solved = await _metrics(client, graph, {**base, "query": "solved"})
        assert {g["key"]: g["value"] for g in solved["groups"]} == {"a": 2.0, "b": 1.0}

        # A completion window keeps the runs completed inside [start_ms, end_ms).
        windowed = await _metrics(
            client,
            graph,
            {**base, "query": "cost", "window": {"start_ms": 0, "end_ms": 15}},
        )
        assert windowed["groups"] == [{"key": "a", "value": 1.0, "trace_count": 1}]

        # No solved run in the population: abstain, never a zero.
        none = await _metrics(
            client,
            graph,
            {**base, "query": "cost", "window": {"start_ms": 25, "end_ms": 35}},
        )
        assert none["groups"] == []
        assert none["abstained"] == "NO_SOLVED_RUNS"

        # A tenant other than the verified one, and an empty window, are refused.
        with pytest.raises(Exception, match="ACCESS_DENIED"):
            await _metrics(client, graph, {"tenant": "other-tenant", "query": "cost"})
        with pytest.raises(Exception, match="INVALID_ARGUMENT"):
            await _metrics(
                client,
                graph,
                {**base, "query": "cost", "window": {"start_ms": 9, "end_ms": 9}},
            )
        await client.tenants.delete(graph)
    finally:
        await client.close()


# spec: MEDULLA-ORCH-VOCAB-R811.1
@pytest.mark.spec("MEDULLA-ORCH-VOCAB-R811.1")
def test_run_metrics_groups_solved_runs_and_refuses_a_foreign_tenant():
    asyncio.run(_scenario(f"run-metrics-{uuid.uuid4().hex[:8]}"))
