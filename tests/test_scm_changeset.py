"""MEDULLA-SCM-R002: the served ``SubmitChangeset`` method.

A three-file repository lands as one changeset; changing two files lands one
more changeset with exactly two path entries whose ``old_blob`` values come
from the first parent. The chain grows by one commit per accepted changeset, a
replay under the same idempotency key returns the original record, and every
refusal leaves the chain unchanged.
"""

from __future__ import annotations

import asyncio
import os
import uuid

import pytest
from conftest import TEST_TENANT, request_context

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
        "message": "change files\n\nSpec: MEDULLA-SCM-R002",
        "paths": paths,
        "trailers": None,
        "run_trace_id": None,
    }


async def _submit(client: MedullaClient, request: dict, *, key: str):
    return await client._send(
        "SubmitChangeset", {"request": request}, idempotency_key=key
    )


async def _refusal(client: MedullaClient, request: dict, *, key: str) -> str:
    try:
        await _submit(client, request, key=key)
    except Exception as exc:
        return str(exc)
    raise AssertionError("SubmitChangeset unexpectedly succeeded")


async def _scenario(graph: str) -> None:
    client = await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
        timeout=30,
    )
    try:
        await client.tenants.create(graph, "Global")
        stem = uuid.uuid4().hex[:8]
        branch = f"feature/scm-{stem}"
        await client._send(
            "CreateBranch", {"tenant": TEST_TENANT, "graph": graph, "name": branch}
        )

        files = {
            name: f"{name} v1 {stem}".encode() for name in ("a.rs", "b.rs", "c.rs")
        }
        digests = {n: (await client.blob.store(b), len(b)) for n, b in files.items()}
        first = await _submit(
            client,
            _request(
                graph,
                branch,
                None,
                [_entry(n, "add", digests[n]) for n in files],
            ),
            key=f"s1-{stem}",
        )
        assert first["replayed"] is False
        assert len(first["changeset"]["changed_paths"]) == 3
        assert first["changeset"]["trailers"]["spec_ids"] == ["MEDULLA-SCM-R002"]
        head = first["branch_head"]

        # Change two of the three files: one changeset, two path entries.
        new_a = f"a.rs v2 {stem}".encode()
        a2 = (await client.blob.store(new_a), len(new_a))
        second = await _submit(
            client,
            _request(
                graph,
                branch,
                head,
                [_entry("a.rs", "modify", a2), _entry("c.rs", "delete", None)],
            ),
            key=f"s2-{stem}",
        )
        record = second["changeset"]
        assert [p["path"] for p in record["changed_paths"]] == ["a.rs", "c.rs"]
        assert record["changed_paths"][0]["old_blob"] == digests["a.rs"][0]
        assert record["changed_paths"][0]["new_blob"] == a2[0]
        assert record["changed_paths"][1]["old_blob"] == digests["c.rs"][0]
        assert record["changed_paths"][1]["new_blob"] is None
        assert record["parents"] == [head]
        assert record["author_principal"]
        assert second["branch_head"] != head

        # A replay under the same key returns the original record.
        again = await _submit(
            client,
            _request(
                graph,
                branch,
                head,
                [_entry("a.rs", "modify", a2), _entry("c.rs", "delete", None)],
            ),
            key=f"s2-{stem}",
        )
        assert again["replayed"] is True
        assert again["changeset"] == record
        assert again["branch_head"] == second["branch_head"]

        # Refusals leave the chain unchanged.
        tip = second["branch_head"]
        missing = ("f" * 64, 3)
        assert "CHANGESET_EMPTY" in await _refusal(
            client, _request(graph, branch, tip, []), key=f"r1-{stem}"
        )
        assert "CHANGESET_INVALID" in await _refusal(
            client,
            _request(graph, branch, tip, [_entry("d.rs", "delete", a2)]),
            key=f"r2-{stem}",
        )
        assert "BLOB_NOT_FOUND" in await _refusal(
            client,
            _request(graph, branch, tip, [_entry("d.rs", "add", missing)]),
            key=f"r3-{stem}",
        )
        assert "CHANGESET_PARENT_NOT_ON_BRANCH" in await _refusal(
            client,
            _request(graph, branch, "e" * 64, [_entry("d.rs", "add", a2)]),
            key=f"r4-{stem}",
        )
        # The chain still ends at the second changeset: its head is a valid base.
        third = await _submit(
            client,
            _request(graph, branch, tip, [_entry("d.rs", "add", a2)]),
            key=f"s3-{stem}",
        )
        assert third["changeset"]["parents"] == [tip]
        await client.tenants.delete(graph)
    finally:
        await client.close()


# spec: MEDULLA-SCM-R002
@pytest.mark.spec("MEDULLA-SCM-R002")
def test_changeset_stores_only_changed_paths():
    asyncio.run(_scenario(f"scm-changeset-{uuid.uuid4().hex[:8]}"))
