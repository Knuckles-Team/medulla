"""MEDULLA-BRANCH-MERGE-R010: line-granularity merge on the served accept path.

Two writers start from the same changeset. The first lands an edit to one
line. The second, still on the old base, edits a different line of the same
file: the engine rebases it by line and the head file holds both edits. A third
writer on the old base edits the line the first one changed: the submission
refuses with ``SOURCE_CONFLICT`` naming the path and the chain is unchanged.
"""

from __future__ import annotations

import asyncio
import os
import uuid

import pytest
from conftest import TEST_TENANT, request_context

from medulla.client import MedullaClient

MODE = 0o100644
PATH = "src/lib.rs"
BASE = "one\ntwo\nthree\nfour\nfive\n"


def _entry(blob: tuple[str, int]) -> dict:
    return {
        "path": PATH,
        "op": "modify",
        "blob": {"digest": blob[0], "size": blob[1]},
        "mode": MODE,
        "renamed_from": None,
    }


def _request(graph: str, branch: str, base: str | None, entry: dict) -> dict:
    return {
        "repo": graph,
        "branch": branch,
        "base_head": base,
        "message": "edit lib\n\nSpec: MEDULLA-BRANCH-MERGE-R010",
        "paths": [entry],
        "trailers": None,
        "run_trace_id": None,
    }


async def _store(client: MedullaClient, text: str) -> tuple[str, int]:
    data = text.encode()
    return await client.blob.store(data), len(data)


async def _scenario(graph: str) -> None:
    client = await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
        timeout=30,
    )
    try:
        await client.tenants.create(graph, "Global")
        stem = uuid.uuid4().hex[:8]
        branch = f"feature/merge-{stem}"
        await client._send(
            "CreateBranch", {"tenant": TEST_TENANT, "graph": graph, "name": branch}
        )

        base_blob = await _store(client, BASE)
        add = dict(_entry(base_blob), op="add")
        first = await client._send(
            "SubmitChangeset",
            {"request": _request(graph, branch, None, add)},
            idempotency_key=f"base-{stem}",
        )
        base_head = first["branch_head"]

        # Writer A edits line one and lands on the base.
        a_blob = await _store(client, BASE.replace("one", "ONE"))
        landed = await client._send(
            "SubmitChangeset",
            {"request": _request(graph, branch, base_head, _entry(a_blob))},
            idempotency_key=f"a-{stem}",
        )
        head = landed["branch_head"]

        # Writer B, still on the old base, edits line five: disjoint hunks merge.
        b_blob = await _store(client, BASE.replace("five", "FIVE"))
        merged = await client._send(
            "SubmitChangeset",
            {"request": _request(graph, branch, base_head, _entry(b_blob))},
            idempotency_key=f"b-{stem}",
        )
        record = merged["changeset"]
        assert record["parents"] == [head]
        (path_entry,) = record["changed_paths"]
        assert path_entry["old_blob"] == a_blob[0]
        assert path_entry["new_blob"] not in (a_blob[0], b_blob[0])
        landed_bytes = await client.blob.fetch(path_entry["new_blob"])
        assert landed_bytes == b"ONE\ntwo\nthree\nfour\nFIVE\n"
        tip = merged["branch_head"]
        assert tip != head

        # Writer C, on the old base, edits line one differently: one shared line
        # conflicts and the chain does not move.
        c_blob = await _store(client, BASE.replace("one", "uno"))
        try:
            await client._send(
                "SubmitChangeset",
                {"request": _request(graph, branch, base_head, _entry(c_blob))},
                idempotency_key=f"c-{stem}",
            )
        except Exception as exc:
            text = str(exc)
            assert "SOURCE_CONFLICT" in text
            assert PATH in text
        else:
            raise AssertionError("an overlapping edit unexpectedly merged")
        # The chain still ends at the merged changeset: its head is a valid base.
        again = await client._send(
            "SubmitChangeset",
            {"request": _request(graph, branch, tip, _entry(c_blob))},
            idempotency_key=f"d-{stem}",
        )
        assert again["changeset"]["parents"] == [tip]
        await client.tenants.delete(graph)
    finally:
        await client.close()


# spec: MEDULLA-BRANCH-MERGE-R010
@pytest.mark.spec("MEDULLA-BRANCH-MERGE-R010")
def test_disjoint_hunks_merge_and_overlap_conflicts():
    asyncio.run(_scenario(f"branch-line-merge-{uuid.uuid4().hex[:8]}"))
