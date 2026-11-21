"""MEDULLA-DECISION-ENGINE-R174.8.6: vocabulary drift refreshes only the changed shard.

The entry point is the served graph write path followed by served ``Discover``. Placement is per
graph: the engine runs four durable shards and the journey uses four graphs whose names land on
four distinct shards, so each graph is one monitored shard. Every graph writes a reference window
of ``Document`` nodes with embeddings; one graph then writes vocabulary its reference never saw.
Two served ``EvaluateDrift`` calls schedule a downtime refresh for that graph alone, and the
served ``RefreshDrift`` re-embeds its nodes, moves its shard generation and records the receipt as
a graph node. A repeat answers the recorded receipt.
"""

from __future__ import annotations

import asyncio
import time
import uuid
from pathlib import Path
from typing import Any

import msgpack
import pytest
from _commons_generation import DIM, connect, restartable_engine
from _harness_envelope import draft_envelope, op
from conftest import TEST_TENANT

from medulla import decision_client as dc

SHARDS = 4
WINDOW = 256
CHUNK = 128


def _shard_of(graph: str) -> int:
    """The engine's per-graph placement (``shard_index`` over the sanitized name)."""
    assert all(c.isalnum() or c in "-_." for c in graph), graph
    value = 0xCBF29CE484222325
    for byte in graph.encode():
        value ^= byte
        value = (value * 0x100000001B3) & 0xFFFFFFFFFFFFFFFF
    return value % SHARDS


def _graphs(suffix: str) -> list[str]:
    """Four graph names that land on four distinct shards."""
    chosen: dict[int, str] = {}
    counter = 0
    while len(chosen) < SHARDS:
        name = f"drift-l4-{suffix}-{counter}"
        chosen.setdefault(_shard_of(name), name)
        counter += 1
    assert sorted(chosen) == list(range(SHARDS))
    return [chosen[shard] for shard in range(SHARDS)]


def _vector(seed: int) -> list[float]:
    return [float((seed + d) % 7) for d in range(DIM)]


async def _write_documents(client: Any, graph: str, phase: int, token: str) -> None:
    """``WINDOW`` documents carrying ``token``, each with its embedding."""
    for start in range(0, WINDOW, CHUNK):
        operations = [
            op(
                ordinal,
                "AddNode",
                {
                    "node_id": f"doc-{phase}-{start + ordinal}",
                    "properties_msgpack": msgpack.packb(
                        {"type": "Document", "text": f"{token} {phase}"}
                    ),
                },
            )
            for ordinal in range(CHUNK)
        ]
        key = f"r174-8-6:{graph}:{phase}:{start}"
        envelope = draft_envelope(graph, key, key, 1, operations)
        results = await client.changes.apply_batch([envelope])
        assert [r["result"] for r in results] == ["applied"], results
        for ordinal in range(CHUNK):
            await client.graph.add_embedding(
                f"doc-{phase}-{start + ordinal}", _vector(start + ordinal)
            )


async def _update_cell(client: Any, cell: str) -> None:
    now = int(time.time() * 1000)
    await client.capacity_leases.update_cell(
        {
            "schema_version": "1",
            "cell": {
                "cell_id": cell,
                "resource_class": "cpu",
                "capacity": 8,
                "reserved_floor": 0,
                "epoch": 1,
                "policy_digest": "sha256:" + "d" * 64,
                "updated_at_ms": now,
            },
            "expected_epoch": None,
            "now_ms": now,
        }
    )


async def _journey(engine: Any, suffix: str) -> None:
    graphs = _graphs(suffix)
    drifting = graphs[1]
    cell = f"cell-{suffix}"
    admin = await connect(engine=engine)
    clients: dict[str, Any] = {}
    try:
        for graph in graphs:
            await admin.tenants.create(graph)
            client = await connect(graph, engine=engine)
            clients[graph] = client
            await _update_cell(client, cell)
            decision = dc.DecisionClient(client=client)
            policy = await decision.log(
                dc.learn_op(
                    TEST_TENANT,
                    "register_drift_policy",
                    logical_graph=graph,
                    region_budget=64,
                )
            )
            assert policy["recorded"] == "drift_policy", policy
            await _write_documents(client, graph, 0, "known")
        for graph in graphs:
            token = "never-seen-before" if graph == drifting else "known"
            await _write_documents(clients[graph], graph, 1, token)

        async def evaluate(graph: str) -> dict[str, Any]:
            decision = dc.DecisionClient(client=clients[graph])
            return await decision.log(
                dc.learn_op(
                    TEST_TENANT,
                    "evaluate_drift",
                    logical_graph=graph,
                    capacity_cell=cell,
                    priority="downtime",
                )
            )

        results: dict[str, dict[str, Any]] = {}
        for graph in graphs:
            await evaluate(graph)
            results[graph] = await evaluate(graph)
        for graph in graphs:
            found = results[graph]
            assert found["recorded"] == "drift", found
            if graph != drifting:
                assert found["firing"] == [], found
                assert found["scheduled"] is None, found
                continue
            assert [f["shard_id"] for f in found["firing"]] == [_shard_of(graph)], found
            assert found["firing"][0]["kinds"] == ["vocabulary_growth"], found
            assert len(found["scheduled"]["plan"]["regions"]) == 1, found

        plan = results[drifting]["scheduled"]
        decision = dc.DecisionClient(client=clients[drifting])
        refresh = dc.learn_op(
            TEST_TENANT,
            "refresh_drift",
            logical_graph=drifting,
            capacity_cell=cell,
            plan_digest=plan["plan_digest"],
        )
        receipt = await decision.log(refresh)
        assert receipt["recorded"] == "drift_refreshed", receipt
        assert receipt["reembedded_nodes"] == 2 * WINDOW, receipt
        shard = _shard_of(drifting)
        assert [s["shard_id"] for s in receipt["shards"]] == [shard], receipt
        swap = receipt["shards"][0]
        assert swap["generation_after"] == swap["generation_before"] + 1, receipt

        # Discover over the refreshed graph is served by one generation.
        found = await clients[drifting].graph.discover(["never-seen-before"], _vector(1), k=5)
        assert found["hits"], found

        # The receipt is a graph node of the same commit as the shard pointer, so a repeat
        # answers it and moves nothing.
        again = await decision.log(refresh)
        assert again == receipt, again
        pointer = await clients[drifting].nodes.properties(f"drift-shard:{drifting}/{shard}")
        assert pointer["generation"] == swap["generation_after"], pointer
        stored = await clients[drifting].nodes.properties(f"drift-refresh:{plan['plan_digest']}")
        assert stored["plan_digest"] == plan["plan_digest"], stored
    finally:
        for client in clients.values():
            await client.close()
        await admin.close()


# spec: MEDULLA-DECISION-ENGINE-R174.8.6
@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R174.8.6")
@pytest.mark.no_engine
def test_vocabulary_drift_refreshes_changed_shards(tmp_path: Path) -> None:
    suffix = uuid.uuid4().hex[:8]
    with restartable_engine(
        tmp_path, "drift", bootstrap=True, extra_env={"MEDULLA_REDB_SHARDS": str(SHARDS)}
    ) as engine:
        asyncio.run(_journey(engine, suffix))
