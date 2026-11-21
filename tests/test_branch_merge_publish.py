"""MEDULLA-BRANCH-MERGE-R007: atomic two-parent merge publication and replay.

Two branches cut from one base each add a different node. Merging one into the
other publishes ONE two-parent commit (the target head first, the source head
second) whose state reads back with both nodes. Merging again returns the
original receipt. A stale caller pin refuses ``MERGE_TARGET_STALE`` and the
target head stays where it was.
"""

from __future__ import annotations

import asyncio
import hashlib
import os
import uuid

import pytest
from conftest import TEST_AGENT_ID, TEST_TENANT, request_context

from medulla.client import MedullaClient


async def _write(client: MedullaClient, graph: str, branch: str, node: str, key: str):
    params = {
        "tenant": TEST_TENANT,
        "graph": graph,
        "branch": branch,
        "mutation": {
            "method": "AddNode",
            "params": {"node_id": node, "properties_msgpack": b"\x80"},
        },
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


async def _read_at(client: MedullaClient, graph: str, branch: str, version: int) -> list:
    snapshot = await client._send(
        "BranchReadAt",
        {
            "tenant": TEST_TENANT,
            "graph": graph,
            "branch": branch,
            "engine_version": version,
        },
    )
    return sorted(node["id"] for node in snapshot["nodes"])


async def _scenario(graph: str) -> None:
    client = await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
        timeout=30,
    )
    try:
        await client.tenants.create(graph, "Global")
        stem = uuid.uuid4().hex[:8]
        left, right = f"feature/left-{stem}", f"feature/right-{stem}"
        for name in (left, right):
            await client._send(
                "CreateBranch", {"tenant": TEST_TENANT, "graph": graph, "name": name}
            )
        l_write = await _write(client, graph, left, f"l-{stem}", f"kl-{stem}")
        r_write = await _write(client, graph, right, f"r-{stem}", f"kr-{stem}")

        # A stale pin refuses and the target head stays where it was.
        try:
            await client._send(
                "BranchMerge", _merge(graph, left, right, merge_base="not-the-base")
            )
        except Exception as exc:
            assert "MERGE_TARGET_STALE" in str(exc)
        else:
            raise AssertionError("a stale pin must refuse")
        assert await _read_at(
            client, graph, right, r_write["commit"]["engine_version"]
        ) == [f"r-{stem}"]

        # Publish: one commit with two parents, target head first.
        receipt = await client._send("BranchMerge", _merge(graph, left, right))
        assert receipt["outcome"] == "merged"
        assert receipt["conflicts"] == []
        assert receipt["replayed"] is False
        commit = receipt["commit"]
        assert commit["parents"] == [r_write["branch_head"], l_write["branch_head"]]
        assert receipt["target_head"] == commit["id"]
        assert receipt["merged_parent"] == l_write["branch_head"]
        assert commit["manifest_digest"] and commit["actor"]
        assert await _read_at(client, graph, right, commit["engine_version"]) == [
            f"l-{stem}",
            f"r-{stem}",
        ]

        # Replay: the original receipt, byte for byte, and no second commit.
        again = await client._send("BranchMerge", _merge(graph, left, right))
        assert again["replayed"] is True
        assert {**again, "replayed": False} == receipt
        await client.tenants.delete(graph)
    finally:
        await client.close()


# spec: MEDULLA-BRANCH-MERGE-R007
@pytest.mark.spec("MEDULLA-BRANCH-MERGE-R007")
def test_atomic_two_parent_publication_and_replay():
    asyncio.run(_scenario(f"branch-merge-pub-{uuid.uuid4().hex[:8]}"))


def _target_graph(graph: str, branch: str) -> str:
    """The branch target graph name (``branch_target_graph``)."""
    key = "/".join(f"{len(part)}:{part}" for part in (TEST_TENANT, graph, branch))
    return f"{graph}~branch~{hashlib.sha256(key.encode()).hexdigest()[:16]}"


# msgpack of {"type": "Doc"}
_DOC = b"\x81\xa4type\xa3Doc"


async def _fast_forward_scenario(graph: str) -> None:
    client = await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
        timeout=30,
    )
    try:
        await client.tenants.create(graph, "Global")
        stem = uuid.uuid4().hex[:8]
        source, target = f"feature/src-{stem}", f"feature/dst-{stem}"
        for name in (source, target):
            await client._send(
                "CreateBranch", {"tenant": TEST_TENANT, "graph": graph, "name": name}
            )
        await client._send(
            "SetPathGrant",
            {
                "request": {
                    "repo": graph,
                    "principal": TEST_AGENT_ID,
                    "prefix": "",
                    "read": True,
                    "write": True,
                }
            },
        )
        node = f"ff-{stem}"
        await client._send(
            "BranchWrite",
            {
                "tenant": TEST_TENANT,
                "graph": graph,
                "branch": source,
                "mutation": {
                    "method": "AddNode",
                    "params": {"node_id": node, "properties_msgpack": _DOC},
                },
            },
            idempotency_key=f"kff-{stem}",
        )
        digest = await client.blob.store(f"file {stem}".encode())
        submitted = await client._send(
            "SubmitChangeset",
            {
                "request": {
                    "repo": graph,
                    "branch": source,
                    "base_head": None,
                    "message": "add a\n\nSpec: MEDULLA-BRANCH-MERGE-R007",
                    "paths": [
                        {
                            "path": "a",
                            "op": "add",
                            "blob": {"digest": digest, "size": len(f"file {stem}")},
                            "mode": 0o100644,
                            "renamed_from": None,
                        }
                    ],
                    "trailers": None,
                    "run_trace_id": None,
                }
            },
            idempotency_key=f"kcs-{stem}",
        )
        graph_of_target = _target_graph(graph, target)
        history = {
            "request": {
                "repo": graph,
                "branch": target,
                "rev": submitted["branch_head"],
                "path": "a",
                "limit": 0,
            }
        }

        # Before: the target reads none of the source's data or history.
        assert not await client._send("HasNode", {"node_id": node}, graph=graph_of_target)
        try:
            before = await client._send("ReadPathHistory", history)
        except Exception:
            before = {"entries": []}
        assert before["entries"] == []

        receipt = await client._send("BranchMerge", _merge(graph, source, target))
        assert receipt["outcome"] == "fast_forward"
        assert receipt["target_head"] == submitted["branch_head"]
        assert receipt["merged_parent"] == submitted["branch_head"]

        # After: the node reads through the target's own data graph (Uql) and
        # the history row through the history read, with no further step.
        found = await client._send(
            "Uql", {"text": "MATCH (:Doc)", "params": {}}, graph=graph_of_target
        )
        assert node in [row["id"] for row in found["rows"]]
        after = await client._send("ReadPathHistory", history)
        assert [entry["commit"] for entry in after["entries"]] == [submitted["branch_head"]]
        await client.tenants.delete(graph)
    finally:
        await client.close()


# spec: MEDULLA-BRANCH-MERGE-R007
@pytest.mark.spec("MEDULLA-BRANCH-MERGE-R007")
def test_fast_forward_target_reads_source_data():
    asyncio.run(_fast_forward_scenario(f"branch-merge-ff-{uuid.uuid4().hex[:8]}"))
