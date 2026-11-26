"""MEDULLA-DECISION-ENGINE-R651.6 served memory journey: remember, recall with a
consolidation plan, consolidate through the gated ``Consolidate`` route, export.

Three episodes of one pattern and one semantic fact are remembered with their
vectors. A recall with ``include_plan`` plans the promotion of the three
episodes (support 3); the promotion commits through the existing route under an
admit receipt; the export then answers a manifest whose digest is stable
across repeated reads and covers the semantic tiers.
"""

from __future__ import annotations

import asyncio
import os
import uuid
from typing import Any

import pytest
from _memory_receipts import benchmark, pin_policy, submit
from conftest import TEST_TENANT, request_context

from medulla import decision_client as dc
from medulla.client import MedullaClient

from .test_consolidate_served import _candidate_digest, _consolidate


def _record(
    prefix: str, name: str, tier: str, text: str, pattern: str | None = None
) -> dict[str, Any]:
    record: dict[str, Any] = {
        "memory_id": f"{prefix}:{name}",
        "tenant": TEST_TENANT,
        "kind": "fact",
        "tier": tier,
        "text": text,
        "timestamp_ms": 1_000,
        "importance": 1.0,
        "token_estimate": 10,
    }
    if pattern is not None:
        record["pattern_key"] = pattern
    return record


async def _remember_and_plan(prefix: str) -> tuple[dict[str, Any], list[str]]:
    client = await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
    )
    try:
        decision = dc.DecisionClient(client=client)
        episodes = [f"{prefix}:run{n}" for n in range(3)]
        for n in range(len(episodes)):
            await decision.memory_remember(
                _record(
                    prefix,
                    f"run{n}",
                    "episodic",
                    f"ran the build {prefix} {n}",
                    pattern=f"{prefix}-build",
                ),
                [0.5, 0.5 + 0.01 * n],
            )
        await decision.memory_remember(
            _record(prefix, "sky", "semantic", f"the sky is blue {prefix}"),
            [1.0, 0.0],
        )
        recalled = await decision.memory_recall(
            {"query_embedding": [0.5, 0.5], "budget": 10_000, "include_plan": True}
        )
        return recalled, episodes
    finally:
        await client.close()


async def _export() -> dict[str, Any]:
    client = await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
    )
    try:
        return await dc.DecisionClient(client=client).memory_export()
    finally:
        await client.close()


# spec: MEDULLA-DECISION-ENGINE-R651.6
@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R651.6")
def test_remember_recall_plan_export(clean_graph):
    prefix = f"mc{uuid.uuid4().hex[:8]}"
    recalled, episodes = asyncio.run(_remember_and_plan(prefix))

    # The plan promotes the three episodes of the pattern and writes nothing.
    plan = recalled["plan"]
    promotion = next(
        item for item in plan["promotions"] if item["pattern_key"] == f"{prefix}-build"
    )
    assert promotion["support_count"] == 3
    assert recalled["plan_digest"]

    # Consolidate through the existing gated route under an admit receipt.
    client = clean_graph
    policy_ref, _ = pin_policy(client)
    nodes = [f"memory:{TEST_TENANT}:{memory_id}" for memory_id in episodes]
    props = {
        "id": f"semantic:{prefix}",
        "pattern_key": f"{prefix}-build",
        "support_count": 3,
    }
    request = benchmark(
        policy_ref,
        f"consolidate-{prefix}",
        True,
        record_id=props["id"],
        change_kind="consolidate",
        baseline={"revision": 0, "digest": "0" * 64},
        candidate={"revision": 1, "digest": _candidate_digest(nodes, props)},
    )
    receipt = submit(client, dc.memory_benchmark_op(request))["state"]["output"][
        "receipt"
    ]
    assert receipt["verdict"] == "admit", receipt
    assert (
        _consolidate(client, nodes, props, [receipt["receipt_digest"]]) == props["id"]
    )

    # The export names the semantic tier and its digest is stable.
    first = asyncio.run(_export())
    second = asyncio.run(_export())
    manifest = first["manifest"]
    assert manifest["record_count"] == len(first["records"])
    assert len(manifest["manifest_digest"]) == 64
    assert second["manifest"]["manifest_digest"] == manifest["manifest_digest"]
    held = {record["memory_id"] for record in first["records"]}
    assert f"{prefix}:sky" in held
