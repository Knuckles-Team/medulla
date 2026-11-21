"""MEDULLA-BRANCH-MERGE-R003: merge base and fast-forward.

Four topologies drive the closed outcome set: linear (fast-forward, then
up-to-date), equal heads, a diamond (both heads moved from one base) and
disjoint heads (branches cut at different base versions). The diamond names its
merge base; disjoint heads refuse with ``BRANCH_NO_COMMON_ANCESTOR`` and never
get an implicit base.
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


async def _create(client: MedullaClient, graph: str, name: str) -> dict:
    return await client._send(
        "CreateBranch", {"tenant": TEST_TENANT, "graph": graph, "name": name}
    )


async def _write(client: MedullaClient, graph: str, branch: str, node: str, key: str):
    params = {
        "tenant": TEST_TENANT,
        "graph": graph,
        "branch": branch,
        "mutation": _add_node(node),
    }
    return await client._send("BranchWrite", params, idempotency_key=key)


def _merge(graph: str, source: str, target: str, **extra) -> dict:
    return {
        "tenant": TEST_TENANT,
        "graph": graph,
        "source": source,
        "target": target,
        **extra,
    }


async def _refusal(client: MedullaClient, params: dict) -> str:
    try:
        await client._send("BranchMerge", params)
    except Exception as exc:
        return str(exc)
    raise AssertionError("BranchMerge unexpectedly succeeded")


async def _scenario(graph: str) -> None:
    client = await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
        timeout=30,
    )
    try:
        await client.tenants.create(graph, "Global")
        stem = uuid.uuid4().hex[:8]
        trunk, feature = f"feature/trunk-{stem}", f"feature/lin-{stem}"
        await _create(client, graph, trunk)
        await _create(client, graph, feature)

        # Linear: the feature moved, the trunk did not.
        first = await _write(client, graph, feature, f"n1-{stem}", f"k1-{stem}")
        second = await _write(client, graph, feature, f"n2-{stem}", f"k2-{stem}")
        behind = await client._send("BranchMerge", _merge(graph, trunk, feature))
        assert behind["outcome"] == "up_to_date"
        assert behind["merged_parent"] is None
        assert behind["target_head"] == second["branch_head"]

        forward = await client._send("BranchMerge", _merge(graph, feature, trunk))
        assert forward["outcome"] == "fast_forward"
        assert forward["source_head"] == second["branch_head"]
        assert forward["target_head"] == second["branch_head"]
        assert forward["merged_parent"] == second["branch_head"]
        assert first["branch_head"] != second["branch_head"]

        # Equal heads: nothing to publish.
        equal = await client._send("BranchMerge", _merge(graph, feature, trunk))
        assert equal["outcome"] == "up_to_date"
        assert equal["merged_parent"] is None

        # Diamond: both heads moved from one base pointer.
        left, right = f"feature/left-{stem}", f"feature/right-{stem}"
        await _create(client, graph, left)
        await _create(client, graph, right)
        await _write(client, graph, left, f"l-{stem}", f"kl-{stem}")
        await _write(client, graph, right, f"r-{stem}", f"kr-{stem}")
        diverged = await client._send("BranchMerge", _merge(graph, left, right))
        assert diverged["outcome"] == "merged"
        assert diverged["merge_base"].startswith(f"{graph}@")

        # A caller-pinned base that is not the computed one is stale.
        stale = await _refusal(
            client, _merge(graph, left, right, merge_base="not-the-base")
        )
        assert "MERGE_TARGET_STALE" in stale

        # Disjoint: a branch cut at another base version shares no ancestor.
        await client._send("AddNode", {"node_id": f"m-{stem}", "properties_msgpack": b"\x80"}, graph=graph)
        late = f"feature/late-{stem}"
        await _create(client, graph, late)
        assert "BRANCH_NO_COMMON_ANCESTOR" in await _refusal(
            client, _merge(graph, late, left)
        )

        # Refusals before any head moves.
        assert "BRANCH_UNKNOWN" in await _refusal(
            client, _merge(graph, f"feature/ghost-{stem}", trunk)
        )
        assert "BRANCH_TENANT_DENIED" in await _refusal(
            client, _merge(graph, feature, trunk, tenant="tenant:other")
        )
        await client.tenants.delete(graph)
    finally:
        await client.close()


# spec: MEDULLA-BRANCH-MERGE-R003
@pytest.mark.spec("MEDULLA-BRANCH-MERGE-R003")
def test_merge_outcomes_up_to_date_and_fast_forward():
    asyncio.run(_scenario(f"branch-merge-{uuid.uuid4().hex[:8]}"))
