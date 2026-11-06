"""MEDULLA-BRANCH-MERGE-R002: branch-scoped mutations against the real server.

A write names its branch in the signed graph reference ``<graph>@branch/<name>``.
It runs on the branch's copy-on-write view and appends one commit to the
branch chain; ``main`` never sees it. A retry under the same idempotency key
returns the original receipt, and an unknown branch refuses before any change.
"""

from __future__ import annotations

import asyncio
import os
import uuid

import msgpack
import pytest
from conftest import TEST_TENANT, request_context

from medulla.client import MedullaClient


def _add_node(node_id: str) -> dict:
    return {"node_id": node_id, "properties_msgpack": msgpack.packb({})}


async def _refusal(client: MedullaClient, graph: str, node_id: str) -> str:
    try:
        await client._send("AddNode", _add_node(node_id), graph=graph)
    except Exception as exc:
        return str(exc)
    raise AssertionError(f"AddNode on {graph!r} unexpectedly succeeded")


async def _scenario(graph: str) -> None:
    client = await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
        timeout=20,
    )
    try:
        await client.tenants.create(graph, "Global")
        name = f"feature/{uuid.uuid4().hex[:8]}"
        params = {"tenant": TEST_TENANT, "graph": graph, "name": name}
        base = await client._send("CreateBranch", params)
        main_before = await client._send("NodeCount", graph=graph)
        target = f"{graph}@branch/{name}"

        first = await client._send(
            "AddNode", _add_node("n-branch"), graph=target, idempotency_key="k1"
        )
        receipt = first["receipt"]
        assert receipt["replayed"] is False
        assert receipt["commit"]["parents"] == [base["head_commit"]]
        assert receipt["branch_head"] == receipt["commit"]["id"]
        assert await client._send("NodeCount", graph=graph) == main_before

        replay = await client._send(
            "AddNode", _add_node("n-branch"), graph=target, idempotency_key="k1"
        )
        assert replay["receipt"]["replayed"] is True
        assert replay["receipt"]["commit"] == receipt["commit"]
        assert replay["result"] == first["result"]

        second = await client._send(
            "AddNode", _add_node("n-two"), graph=target, idempotency_key="k2"
        )
        assert second["receipt"]["commit"]["parents"] == [receipt["branch_head"]]
        assert await client._send("NodeCount", graph=graph) == main_before

        assert "BRANCH_UNKNOWN" in await _refusal(
            client, f"{graph}@branch/absent-{uuid.uuid4().hex[:6]}", "n-x"
        )
        assert await client._send("NodeCount", graph=graph) == main_before
        await client.tenants.delete(graph)
    finally:
        await client.close()


# spec: MEDULLA-BRANCH-MERGE-R002
@pytest.mark.spec("MEDULLA-BRANCH-MERGE-R002")
def test_branch_write_invisible_until_merge():
    asyncio.run(_scenario(f"branch-write-{uuid.uuid4().hex[:8]}"))
