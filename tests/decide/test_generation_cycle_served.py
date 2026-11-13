"""MEDULLA-DECISION-ENGINE-R174.6: the generation cycle over the wire.

The entry point is the served ``DecisionLog`` method (``learn`` op,
``run_generation_cycle`` write). The shadow graph holds 1,000 nodes written
through the served change-envelope write. The first submission re-embeds one
256-node batch and reports the cycle ``reembedding``. The engine is then
restarted on its durable store, and the same submission resumes from the
checkpoint, finishes the re-embedding and activates (the first generation of a
logical graph bootstraps). A served ``Discover`` over the new generation, asked
with a vector alone, then returns the nodes the cycle embedded.
"""

from __future__ import annotations

import asyncio
import sys
import time
import uuid
from pathlib import Path
from typing import Any

import msgpack
import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from _commons_generation import (
    DIM,
    Engine,
    connect,
    restartable_engine,
)
from _harness_envelope import draft_envelope, op
from conftest import TEST_TENANT

from medulla import decision_client as dc

NODES = 1000
CHUNK = 250
BATCH = 256
MODEL = {"id": "cycle-model", "revision": "1", "dimension": DIM}


async def _write_nodes(client: Any, graph: str) -> None:
    """The shadow graph's ``Doc`` nodes, in chunks of ``CHUNK`` add-node ops."""
    for start in range(0, NODES, CHUNK):
        operations = [
            op(
                ordinal,
                "AddNode",
                {
                    "node_id": f"doc-{start + ordinal}",
                    "properties_msgpack": msgpack.packb(
                        {"type": "Doc", "text": f"doc-{start + ordinal}"}
                    ),
                },
            )
            for ordinal in range(CHUNK)
        ]
        key = f"r174-6:docs:{start}"
        envelope = draft_envelope(graph, key, key, 1, operations)
        results = await client.changes.apply_batch([envelope])
        assert [result["status"] for result in results] == ["applied"], results


async def _update_cell(client: Any, cell: str) -> None:
    now = int(time.time() * 1000)
    await client.capacity_leases.update_cell(
        {
            "schema_version": "1",
            "cell": {
                "cell_id": cell,
                "resource_class": "llm_embedding",
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


async def _space_read(decision: dc.DecisionClient, logical: str) -> dict[str, Any]:
    """The logical graph's active generation and the model's space digest."""
    return await decision.log(
        {
            "op": "generation_space",
            "tenant_id": TEST_TENANT,
            "logical": logical,
            "model": MODEL,
        }
    )


def _cycle_op(
    logical: str,
    shadow: str,
    cell: str,
    shadow_space: str,
    max_batches: int | None,
) -> dict[str, Any]:
    return dc.learn_op(
        TEST_TENANT,
        "run_generation_cycle",
        request={
            "model_ref": MODEL,
            "evaluation": {
                "logical": logical,
                "active_graph": logical,
                "active_space": "none",
                "shadow_graph": shadow,
                "shadow_space": shadow_space,
                "items": [],
                "top_k": 5,
                "min_eval_items": 5,
                "max_score_psi_q16": 64 << 16,
            },
            "capacity_cell": cell,
            "max_batches": max_batches,
        },
    )


# spec: MEDULLA-DECISION-ENGINE-R174.6
@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R174.6")
@pytest.mark.no_engine
def test_cycle_resumes_and_activates(tmp_path: Path) -> None:
    suffix = uuid.uuid4().hex[:8]
    logical = f"cycle-logical-{suffix}"
    shadow = f"cycle-shadow-{suffix}"
    cell = f"cell-{suffix}"

    async def first_run(engine: Engine) -> tuple[str, str]:
        client = await connect(engine=engine)
        try:
            await client.tenants.create(logical)
            await client.tenants.create(shadow)
            await _update_cell(client, cell)
            decision = dc.DecisionClient(client=client)
            space = (await _space_read(decision, logical))["model_space_digest"]
        finally:
            await client.close()
        client = await connect(shadow, engine=engine)
        try:
            await _write_nodes(client, shadow)
            decision = dc.DecisionClient(client=client)
            first = await decision.log(_cycle_op(logical, shadow, cell, space, 1))
            assert first["recorded"] == "cycle", first
            assert first["cycle"]["state"] == "reembedding", first
            assert (first["batches"], first["reembedded_nodes"]) == (1, BATCH), first
            assert first["next_node"] == BATCH, first
            assert first["receipt"] is None, first
            read = await _space_read(decision, logical)
            assert read["active"] is None, "nothing is served before activation"
            return space, first["job_id"]
        finally:
            await client.close()

    async def second_run(engine: Engine, space: str, job_id: str) -> None:
        client = await connect(shadow, engine=engine)
        try:
            decision = dc.DecisionClient(client=client)
            done = await decision.log(_cycle_op(logical, shadow, cell, space, None))
            assert done["cycle"]["state"] == "activated", done
            assert done["reembedded_nodes"] == NODES, done
            assert done["job_id"] == job_id, "one job per graph and model revision"
            # The restart resumed at the checkpoint: the nodes embedded before
            # it are never embedded again.
            assert done["batches"] == -(-NODES // BATCH), done
            read = await _space_read(decision, logical)
            assert read["active"]["graph"] == shadow, read
            assert read["active"]["space_digest"] == space, read
            # Discover over the new generation, asked by vector alone, finds
            # nodes: only the cycle's embeddings can answer it.
            found = await client.discover([], [1.0] + [0.0] * (DIM - 1), k=5)
            hits = found["hits"]
            assert hits, found
            assert all(hit["type"] == "Doc" for hit in hits), hits
        finally:
            await client.close()

    with restartable_engine(tmp_path, "first", bootstrap=True) as engine:
        space, job_id = asyncio.run(first_run(engine))
    with restartable_engine(tmp_path, "restart", bootstrap=False) as engine:
        asyncio.run(second_run(engine, space, job_id))
