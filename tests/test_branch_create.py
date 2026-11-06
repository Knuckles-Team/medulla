"""MEDULLA-BRANCH-MERGE-R001: CreateBranch against the real server binary.

A branch is one row in the branch registry owner file. Each of the five
refusals returns its registered code and leaves no registry row: a refused
attempt under a valid name is followed by a create of that same name, which
would refuse ``BRANCH_EXISTS`` had the refused attempt written a row.
"""

from __future__ import annotations

import asyncio
import os
import uuid

import pytest
from conftest import TEST_TENANT, request_context

from medulla.client import MedullaClient


def _create(client: MedullaClient, graph: str, name: str, **fields):
    params = {"tenant": TEST_TENANT, "graph": graph, "name": name, **fields}
    return client._send("CreateBranch", params)


async def _refusal(client: MedullaClient, graph: str, name: str, **fields) -> str:
    try:
        await _create(client, graph, name, **fields)
    except Exception as exc:
        return str(exc)
    raise AssertionError(f"CreateBranch {name!r} {fields} unexpectedly succeeded")


async def _scenario(graph: str) -> None:
    client = await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
        timeout=20,
    )
    try:
        await client.tenants.create(graph, "Global")
        stem = uuid.uuid4().hex[:8]
        created = await _create(client, graph, f"feature/{stem}")
        assert created["name"] == f"feature/{stem}"
        assert created["graph"] == graph and created["tenant"] == TEST_TENANT
        assert created["deleted"] is False
        assert "BRANCH_EXISTS" in await _refusal(client, graph, f"feature/{stem}")

        assert "BRANCH_NAME_INVALID" in await _refusal(client, graph, "Bad Name")
        assert "BRANCH_NAME_RESERVED" in await _refusal(client, graph, "main")
        assert "BRANCH_NAME_RESERVED" in await _refusal(client, graph, "eg/x")

        late = f"late-{stem}"
        refused = await _refusal(
            client, graph, late, base_engine_version=created["base_version"] + 10**9
        )
        assert "BRANCH_BASE_UNKNOWN" in refused
        foreign = f"foreign-{stem}"
        params = {"tenant": "tenant:other", "graph": graph, "name": foreign}
        try:
            await client._send("CreateBranch", params)
            raise AssertionError("a foreign tenant created a branch")
        except AssertionError:
            raise
        except Exception as exc:
            assert "BRANCH_TENANT_DENIED" in str(exc)
        # Neither refused attempt left a row: the same names create now.
        for name in (late, foreign):
            assert (await _create(client, graph, name))["name"] == name
        await client.tenants.delete(graph)
    finally:
        await client.close()


# spec: MEDULLA-BRANCH-MERGE-R001
@pytest.mark.spec("MEDULLA-BRANCH-MERGE-R001")
def test_branch_create_and_refusals():
    asyncio.run(_scenario(f"branch-create-{uuid.uuid4().hex[:8]}"))
