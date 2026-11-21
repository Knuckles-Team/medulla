"""MEDULLA-SCM-R004: the served ``ReadTree`` method.

A ten-revision chain is submitted through ``SubmitChangeset``; a subtree read
at every revision equals the replay of the submitted changesets computed here
from the receipts. The checkpoint path (a stored snapshot every
``tree_checkpoint_interval`` changesets) is bound by the Rust tests of
``medulla-compute`` and ``medulla-h-ingest``, which write checkpoints at an
interval of four and ten and assert identical answers.
"""

from __future__ import annotations

import asyncio
import os
import uuid

import pytest
from conftest import TEST_AGENT_ID, TEST_TENANT, request_context

from medulla.client import MedullaClient

MODE = 0o100644


def _entry(path: str, op: str, blob: tuple[str, int] | None) -> dict:
    return {
        "path": path,
        "op": op,
        "blob": None if blob is None else {"digest": blob[0], "size": blob[1]},
        "mode": MODE,
        "renamed_from": None,
    }


def _request(graph: str, branch: str, base: str | None, paths: list[dict]) -> dict:
    return {
        "repo": graph,
        "branch": branch,
        "base_head": base,
        "message": "change files\n\nSpec: MEDULLA-SCM-R004",
        "paths": paths,
        "trailers": None,
        "run_trace_id": None,
    }


def _read(graph: str, branch: str, rev: str, prefix: str = "", depth: int = 0) -> dict:
    return {
        "request": {
            "repo": graph,
            "branch": branch,
            "rev": rev,
            "prefix": prefix,
            "depth": depth,
        }
    }


def _replay(history: list[dict], prefix: str, depth: int) -> list[dict]:
    """The full replay of submitted changesets, filtered like ``ReadTree``."""
    tree: dict[str, tuple[str, int]] = {}
    for changeset in history:
        for changed in changeset["changed_paths"]:
            if changed["op"] == "delete" or changed["new_blob"] is None:
                tree.pop(changed["path"], None)
            else:
                tree[changed["path"]] = (changed["new_blob"], changed["mode"])
    out = []
    for path in sorted(tree):
        if prefix and path != prefix and not path.startswith(prefix + "/"):
            continue
        rest = path[len(prefix) :] if prefix else path
        if depth and len([s for s in rest.split("/") if s]) > depth:
            continue
        out.append({"path": path, "blob_digest": tree[path][0], "mode": tree[path][1]})
    return out


async def _scenario(graph: str) -> None:
    client = await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
        timeout=30,
    )
    try:
        await client.tenants.create(graph, "Global")
        stem = uuid.uuid4().hex[:8]
        branch = f"feature/tree-{stem}"
        await client._send(
            "CreateBranch", {"tenant": TEST_TENANT, "graph": graph, "name": branch}
        )
        # MEDULLA-SCM-R007: the source methods deny by default.
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

        async def blob(text: str) -> tuple[str, int]:
            data = f"{text} {stem}".encode()
            return (await client.blob.store(data), len(data))

        history: list[dict] = []
        revs: list[str] = []
        head: str | None = None
        for n in range(10):
            if n == 0:
                paths = [
                    _entry("src/a.rs", "add", await blob("a0")),
                    _entry("src/deep/b.rs", "add", await blob("b0")),
                    _entry("README", "add", await blob("r0")),
                ]
            elif n == 3:
                paths = [_entry("src/a.rs", "modify", await blob("a3"))]
            elif n == 5:
                paths = [_entry("src/deep/b.rs", "delete", None)]
            else:
                paths = [_entry(f"src/f{n}.rs", "add", await blob(f"f{n}"))]
            receipt = await client._send(
                "SubmitChangeset",
                {"request": _request(graph, branch, head, paths)},
                idempotency_key=f"t{n}-{stem}",
            )
            history.append(receipt["changeset"])
            head = receipt["branch_head"]
            revs.append(head)

        for index, rev in enumerate(revs):
            for prefix, depth in [("", 0), ("src", 0), ("src", 1), ("src/deep", 0)]:
                answer = await client._send(
                    "ReadTree", _read(graph, branch, rev, prefix, depth)
                )
                assert answer["entries"] == _replay(history[: index + 1], prefix, depth)

        # A revision that is not on the chain, and an out-of-bound depth, refuse.
        for request, code in [
            (_read(graph, branch, "e" * 64), "REVISION_NOT_ON_CHAIN"),
            (_read(graph, branch, revs[0], depth=65), "CHANGESET_INVALID"),
        ]:
            with pytest.raises(Exception, match=code):
                await client._send("ReadTree", request)
        await client.tenants.delete(graph)
    finally:
        await client.close()


# spec: MEDULLA-SCM-R004
@pytest.mark.spec("MEDULLA-SCM-R004")
def test_tree_at_revision_matches_full_replay():
    asyncio.run(_scenario(f"scm-tree-{uuid.uuid4().hex[:8]}"))
