"""MEDULLA-BRANCH-MERGE-R002: branch-scoped mutations on one commit chain.

A write on a branch shows on that branch's target graph, stays invisible on
``main`` and on a sibling branch, appends exactly one commit whose parent is the
prior head, and replays under its idempotency key with the original receipt.
Every refusal is decided before any graph change.
"""

from __future__ import annotations

import asyncio
import os
import uuid

import pytest
from conftest import TEST_TENANT, request_context

from medulla.client import MedullaClient


def _add_node(node_id: str) -> dict:
    return {
        "method": "AddNode",
        "params": {"node_id": node_id, "properties_msgpack": b"\x80"},
    }


async def _write(
    client: MedullaClient,
    graph: str,
    branch: str,
    mutation: dict,
    *,
    key: str,
    tenant: str = TEST_TENANT,
):
    params = {"tenant": tenant, "graph": graph, "branch": branch, "mutation": mutation}
    return await client._send("BranchWrite", params, idempotency_key=key)


async def _has(client: MedullaClient, graph: str, node_id: str) -> bool:
    return bool(await client._send("HasNode", {"node_id": node_id}, graph=graph))


async def _refusal(client: MedullaClient, *args, **kwargs) -> str:
    try:
        await _write(client, *args, **kwargs)
    except Exception as exc:
        return str(exc)
    raise AssertionError(f"BranchWrite {args} unexpectedly succeeded")


async def _scenario(graph: str) -> None:
    client = await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
        timeout=30,
    )
    try:
        await client.tenants.create(graph, "Global")
        stem = uuid.uuid4().hex[:8]
        one, two = f"feature/a-{stem}", f"feature/b-{stem}"
        for name in (one, two):
            await client._send(
                "CreateBranch", {"tenant": TEST_TENANT, "graph": graph, "name": name}
            )

        node_a, node_b = f"node-a-{stem}", f"node-b-{stem}"
        first = await _write(client, graph, one, _add_node(node_a), key=f"k1-{stem}")
        assert first["replayed"] is False
        assert first["branch_head"] == first["commit"]["id"]
        assert len(first["commit"]["parents"]) == 1
        target = first["target_graph"]
        assert target != graph

        # Shows on its own branch; invisible on main and on the sibling branch.
        assert await _has(client, target, node_a)
        assert not await _has(client, graph, node_a)
        sibling = await _write(client, graph, two, _add_node(node_b), key=f"k2-{stem}")
        assert sibling["target_graph"] != target
        assert not await _has(client, sibling["target_graph"], node_a)
        assert not await _has(client, target, node_b)
        assert not await _has(client, graph, node_b)

        # The same key replays the original receipt and writes nothing new.
        again = await _write(client, graph, one, _add_node(node_a), key=f"k1-{stem}")
        assert again["commit"] == first["commit"]
        assert again["branch_head"] == first["branch_head"]
        assert again["target_graph"] == first["target_graph"]

        # A second write chains on the prior head.
        second = await _write(
            client, graph, one, _add_node(f"{node_a}-2"), key=f"k3-{stem}"
        )
        assert second["commit"]["parents"] == [first["branch_head"]]
        assert second["commit"]["engine_version"] >= first["commit"]["engine_version"]

        # Refusals precede any graph change.
        ghost = f"feature/ghost-{stem}"
        assert "BRANCH_UNKNOWN" in await _refusal(
            client, graph, ghost, _add_node("x"), key=f"k4-{stem}"
        )
        assert "BRANCH_UNKNOWN" in await _refusal(
            client, graph, "main", _add_node("x"), key=f"k5-{stem}"
        )
        assert "BRANCH_TENANT_DENIED" in await _refusal(
            client, graph, one, _add_node("x"), key=f"k6-{stem}", tenant="tenant:other"
        )
        read = {"method": "HasNode", "params": {"node_id": node_a}}
        assert "BRANCH_WRITE_INVALID" in await _refusal(
            client, graph, one, read, key=f"k7-{stem}"
        )
        nested = {"method": "DeleteGraph", "params": {"graph_name": graph}}
        assert "BRANCH_WRITE_INVALID" in await _refusal(
            client, graph, one, nested, key=f"k8-{stem}"
        )
        assert not await _has(client, graph, "x")
        await client.tenants.delete(graph)
    finally:
        await client.close()


# spec: MEDULLA-BRANCH-MERGE-R002
@pytest.mark.spec("MEDULLA-BRANCH-MERGE-R002")
def test_branch_write_invisible_until_merge():
    asyncio.run(_scenario(f"branch-write-{uuid.uuid4().hex[:8]}"))
