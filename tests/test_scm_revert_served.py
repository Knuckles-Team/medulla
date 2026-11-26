"""MEDULLA-SCM-R009.1: the served ``RevertChangeset`` method.

The inverse of the newest changeset lands as one new changeset through the
accept path of ``SubmitChangeset``: a modified path takes its old blob back and
an added path is deleted. Asking again answers the same revert with ``replayed``
and appends nothing. A changeset that later changesets touch refuses
``REVERT_HAS_DEPENDENTS`` and one that is not on the chain
``REVISION_NOT_ON_CHAIN``.
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
        "message": "change files\n\nSpec: MEDULLA-SCM-R009.1",
        "paths": paths,
        "trailers": None,
        "run_trace_id": None,
    }


async def _submit(client: MedullaClient, request: dict, key: str) -> dict:
    return await client._send(
        "SubmitChangeset", {"request": request}, idempotency_key=key
    )


async def _revert(
    client: MedullaClient, graph: str, branch: str, changeset: str, key: str
) -> dict:
    return await client._send(
        "RevertChangeset",
        {"request": {"repo": graph, "branch": branch, "changeset": changeset}},
        idempotency_key=key,
    )


async def _refusal(
    client: MedullaClient, graph: str, branch: str, changeset: str, key: str
) -> str:
    try:
        await _revert(client, graph, branch, changeset, key)
    except Exception as exc:
        return str(exc)
    raise AssertionError("RevertChangeset unexpectedly succeeded")


async def _scenario(graph: str) -> None:
    client = await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
        timeout=30,
    )
    try:
        await client.tenants.create(graph, "Global")
        stem = uuid.uuid4().hex[:8]
        branch = f"feature/revert-{stem}"
        await client._send(
            "CreateBranch", {"tenant": TEST_TENANT, "graph": graph, "name": branch}
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
        a1_bytes = f"a v1 {stem}".encode()
        a2_bytes = f"a v2 {stem}".encode()
        c_bytes = f"c v1 {stem}".encode()
        a1 = (await client.blob.store(a1_bytes), len(a1_bytes))
        a2 = (await client.blob.store(a2_bytes), len(a2_bytes))
        c1 = (await client.blob.store(c_bytes), len(c_bytes))

        first = await _submit(
            client,
            _request(graph, branch, None, [_entry("a.rs", "add", a1)]),
            f"s1-{stem}",
        )
        second = await _submit(
            client,
            _request(
                graph,
                branch,
                first["branch_head"],
                [_entry("a.rs", "modify", a2), _entry("c.rs", "add", c1)],
            ),
            f"s2-{stem}",
        )
        target = second["changeset"]["id"]

        # The inverse: the modified path takes its old blob back, the added one goes.
        reverted = await _revert(client, graph, branch, target, f"v1-{stem}")
        assert reverted["replayed"] is False
        record = reverted["changeset"]
        assert record["message"].split("\n")[0] == f"Revert changeset {target}"
        assert record["parents"] == [second["branch_head"]]
        by_path = {p["path"]: p for p in record["changed_paths"]}
        assert set(by_path) == {"a.rs", "c.rs"}
        assert by_path["a.rs"]["new_blob"] == a1[0]
        assert by_path["c.rs"]["new_blob"] is None
        assert reverted["branch_head"] != second["branch_head"]

        # The same revert again answers the revert already on the chain.
        again = await _revert(client, graph, branch, target, f"v2-{stem}")
        assert again["replayed"] is True
        assert again["changeset"]["id"] == record["id"]

        # Refusals append nothing.
        assert "REVERT_HAS_DEPENDENTS" in await _refusal(
            client, graph, branch, first["changeset"]["id"], f"v3-{stem}"
        )
        assert "REVISION_NOT_ON_CHAIN" in await _refusal(
            client, graph, branch, "e" * 64, f"v4-{stem}"
        )
        await client.tenants.delete(graph)
    finally:
        await client.close()


# spec: MEDULLA-SCM-R009.1
@pytest.mark.spec("MEDULLA-SCM-R009.1")
def test_revert_changeset_restores_the_tree_and_is_idempotent():
    asyncio.run(_scenario(f"scm-revert-{uuid.uuid4().hex[:8]}"))
