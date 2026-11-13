"""MEDULLA-BRANCH-MERGE-R006: branch diff and time-travel reads.

Three writes on one branch make a three-version chain. Each version reads as
the branch stood at it, the diff between two versions equals the commits
between them, and a version no commit produced, a foreign tenant and an
unknown branch refuse with their typed codes.
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


def _remove_node(node_id: str) -> dict:
    return {"method": "RemoveNode", "params": {"node_id": node_id}}


async def _write(
    client: MedullaClient, graph: str, branch: str, mutation: dict, key: str
):
    params = {
        "tenant": TEST_TENANT,
        "graph": graph,
        "branch": branch,
        "mutation": mutation,
    }
    return await client._send("BranchWrite", params, idempotency_key=key)


def _read_at(graph: str, branch: str, version: int, tenant: str = TEST_TENANT) -> dict:
    return {
        "tenant": tenant,
        "graph": graph,
        "branch": branch,
        "engine_version": version,
    }


def _diff(graph: str, branch: str, **extra) -> dict:
    return {"tenant": TEST_TENANT, "graph": graph, "branch": branch, **extra}


async def _refusal(client: MedullaClient, method: str, params: dict) -> str:
    try:
        await client._send(method, params)
    except Exception as exc:
        return str(exc)
    raise AssertionError(f"{method} unexpectedly succeeded")


def _ids(snapshot: dict) -> list[str]:
    return [node["id"] for node in snapshot["nodes"]]


async def _scenario(graph: str) -> None:
    client = await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
        timeout=30,
    )
    try:
        await client.tenants.create(graph, "Global")
        stem = uuid.uuid4().hex[:8]
        branch = f"feature/tt-{stem}"
        await client._send(
            "CreateBranch", {"tenant": TEST_TENANT, "graph": graph, "name": branch}
        )

        a, b = f"a-{stem}", f"b-{stem}"
        one = await _write(client, graph, branch, _add_node(a), f"k1-{stem}")
        two = await _write(client, graph, branch, _add_node(b), f"k2-{stem}")
        three = await _write(client, graph, branch, _remove_node(a), f"k3-{stem}")
        versions = [r["commit"]["engine_version"] for r in (one, two, three)]
        assert versions == sorted(set(versions)), "one distinct version per commit"
        v1, v2, v3 = versions

        # Each version reads as the branch stood at it.
        at_1 = await client._send("BranchReadAt", _read_at(graph, branch, v1))
        assert _ids(at_1) == [a]
        assert at_1["commit"]["id"] == one["branch_head"]
        at_2 = await client._send("BranchReadAt", _read_at(graph, branch, v2))
        assert _ids(at_2) == sorted([a, b])
        at_3 = await client._send("BranchReadAt", _read_at(graph, branch, v3))
        assert _ids(at_3) == [b]

        # The diff equals the commits between the two versions.
        feed = await client._send("BranchDiff", _diff(graph, branch, from_version=v1))
        assert [c["id"] for c in feed["commits"]] == [
            two["branch_head"],
            three["branch_head"],
        ]
        shape = [
            (e["entity"]["id"], e["operation"], e["image_ref"]) for e in feed["entries"]
        ]
        assert shape == [
            (b, "added", f"commit:{two['branch_head']}"),
            (a, "deleted", f"commit:{three['branch_head']}"),
        ]
        assert feed["next_offset"] is None and feed["to_version"] == v3

        everything = await client._send("BranchDiff", _diff(graph, branch))
        assert [e["operation"] for e in everything["entries"]] == [
            "added",
            "added",
            "deleted",
        ]
        bounded = await client._send(
            "BranchDiff", _diff(graph, branch, to_version=v2, offset=1)
        )
        assert [e["entity"]["id"] for e in bounded["entries"]] == [b]

        # Typed refusals.
        assert "BRANCH_VERSION_NOT_ON_CHAIN" in await _refusal(
            client, "BranchReadAt", _read_at(graph, branch, v3 + 1000)
        )
        assert "BRANCH_VERSION_NOT_ON_CHAIN" in await _refusal(
            client, "BranchDiff", _diff(graph, branch, from_version=v3 + 1000)
        )
        assert "BRANCH_TENANT_DENIED" in await _refusal(
            client, "BranchReadAt", _read_at(graph, branch, v1, tenant="tenant:other")
        )
        assert "BRANCH_UNKNOWN" in await _refusal(
            client, "BranchDiff", _diff(graph, f"feature/none-{stem}")
        )
        await client.tenants.delete(graph)
    finally:
        await client.close()


# spec: MEDULLA-BRANCH-MERGE-R006
@pytest.mark.spec("MEDULLA-BRANCH-MERGE-R006")
def test_as_of_reads_and_diff():
    asyncio.run(_scenario(f"branch-tt-{uuid.uuid4().hex[:8]}"))
