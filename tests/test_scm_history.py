"""MEDULLA-SCM-R005.1: the served ``ReadPathHistory`` method.

Five changesets are submitted through ``SubmitChangeset``; the history of one
path is read at two revisions and the order, the ``limit`` cap and the
``CHANGESET_INVALID`` refusal above the maximum are asserted.
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


def _submit(
    graph: str, branch: str, base: str | None, n: int, paths: list[dict]
) -> dict:
    return {
        "request": {
            "repo": graph,
            "branch": branch,
            "base_head": base,
            "message": f"change {n}\n\nSpec: MEDULLA-SCM-R005.1",
            "paths": paths,
            "trailers": None,
            "run_trace_id": None,
        }
    }


def _history(graph: str, branch: str, rev: str, path: str, limit: int = 0) -> dict:
    return {
        "request": {
            "repo": graph,
            "branch": branch,
            "rev": rev,
            "path": path,
            "limit": limit,
        }
    }


async def _scenario(graph: str) -> None:
    client = await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
        timeout=30,
    )
    try:
        await client.tenants.create(graph, "Global")
        stem = uuid.uuid4().hex[:8]
        branch = f"feature/history-{stem}"
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

        # Changeset n touches `a` for n in 0, 1, 3 and `b` for n in 2, 4.
        plan = [
            [_entry("a", "add", await blob("a0"))],
            [_entry("a", "modify", await blob("a1"))],
            [_entry("b", "add", await blob("b2"))],
            [_entry("a", "delete", None)],
            [_entry("b", "modify", await blob("b4"))],
        ]
        revs: list[str] = []
        changesets: list[dict] = []
        head: str | None = None
        for n, paths in enumerate(plan):
            receipt = await client._send(
                "SubmitChangeset",
                _submit(graph, branch, head, n, paths),
                idempotency_key=f"h{n}-{stem}",
            )
            changesets.append(receipt["changeset"])
            head = receipt["branch_head"]
            revs.append(head)

        # At the head, `a` lists changesets 3, 1, 0 newest first.
        newest = await client._send(
            "ReadPathHistory", _history(graph, branch, revs[4], "a")
        )
        assert [e["commit"] for e in newest["entries"]] == [revs[3], revs[1], revs[0]]
        assert [e["op"] for e in newest["entries"]] == ["delete", "modify", "add"]
        assert [e["changeset_id"] for e in newest["entries"]] == [
            changesets[3]["id"],
            changesets[1]["id"],
            changesets[0]["id"],
        ]

        # At revision 2 only the changesets at or below it are listed.
        older = await client._send(
            "ReadPathHistory", _history(graph, branch, revs[2], "a")
        )
        assert [e["commit"] for e in older["entries"]] == [revs[1], revs[0]]

        # The limit caps the list; 1000 is the maximum and 1001 refuses.
        capped = await client._send(
            "ReadPathHistory", _history(graph, branch, revs[4], "a", limit=1)
        )
        assert [e["commit"] for e in capped["entries"]] == [revs[3]]
        with pytest.raises(Exception, match="CHANGESET_INVALID"):
            await client._send(
                "ReadPathHistory", _history(graph, branch, revs[4], "a", limit=1001)
            )
        with pytest.raises(Exception, match="REVISION_NOT_ON_CHAIN"):
            await client._send(
                "ReadPathHistory", _history(graph, branch, "e" * 64, "a")
            )
        await client.tenants.delete(graph)
    finally:
        await client.close()


# spec: MEDULLA-SCM-R005.1
@pytest.mark.spec("MEDULLA-SCM-R005.1")
def test_path_history_lists_touching_changesets():
    asyncio.run(_scenario(f"scm-history-{uuid.uuid4().hex[:8]}"))


async def _blame_scenario(graph: str) -> None:
    client = await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
        timeout=30,
    )
    try:
        await client.tenants.create(graph, "Global")
        stem = uuid.uuid4().hex[:8]
        branch = f"feature/blame-{stem}"
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

        async def blob(data: bytes) -> tuple[str, int]:
            return (await client.blob.store(data), len(data))

        original = b"one\ntwo\nthree\nfour\nfive\n"
        modified = b"one\nTWO\nthree\nFOUR\nfive\n"
        steps = [
            [_entry("src/a.txt", "add", await blob(original))],
            [_entry("src/a.txt", "modify", await blob(modified))],
            [
                {
                    **_entry("src/b.txt", "rename", await blob(modified)),
                    "renamed_from": "src/a.txt",
                },
            ],
            [_entry("img.bin", "add", await blob(b"\x00\x01\x02\x00"))],
        ]
        revs: list[str] = []
        head: str | None = None
        for n, paths in enumerate(steps):
            receipt = await client._send(
                "SubmitChangeset",
                _submit(graph, branch, head, n, paths),
                idempotency_key=f"b{n}-{stem}",
            )
            head = receipt["branch_head"]
            revs.append(head)

        def ask(rev: str, path: str) -> dict:
            return {
                "request": {"repo": graph, "branch": branch, "rev": rev, "path": path}
            }

        # The rename keeps the origins: lines 1 and 3 come from the modify.
        answer = await client._send("ReadBlame", ask(revs[2], "src/b.txt"))
        assert [
            (r["start_line"], r["line_count"], r["commit"]) for r in answer["runs"]
        ] == [
            (0, 1, revs[0]),
            (1, 1, revs[1]),
            (2, 1, revs[0]),
            (3, 1, revs[1]),
            (4, 1, revs[0]),
        ]
        with pytest.raises(Exception, match="BLAME_BINARY"):
            await client._send("ReadBlame", ask(revs[3], "img.bin"))
        with pytest.raises(Exception, match="BLAME_PATH_ABSENT"):
            await client._send("ReadBlame", ask(revs[3], "src/a.txt"))
        await client.tenants.delete(graph)
    finally:
        await client.close()


# spec: MEDULLA-SCM-R005.2
@pytest.mark.spec("MEDULLA-SCM-R005.2")
def test_blame_attributes_lines_across_rename():
    asyncio.run(_blame_scenario(f"scm-blame-{uuid.uuid4().hex[:8]}"))
