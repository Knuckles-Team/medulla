"""MEDULLA-SCM-R007.1: ListPathGrants reads one principal's grants in a tenant.

An administrator sets grants for a reader on two repositories. The reader lists its
own grants with a read scope only, and is refused another principal's. A principal
that holds no grants, and a repository that holds none, answer an empty list. A grant
with both bits false is a stored explicit deny and lists.
"""

from __future__ import annotations

import asyncio
import os
import uuid

import pytest
from conftest import request_context

from medulla.client import MedullaClient

READER = "grant-reader-agent"


def _grant(repo: str, prefix: str, read: bool, write: bool) -> dict:
    return {
        "request": {
            "repo": repo,
            "principal": READER,
            "prefix": prefix,
            "read": read,
            "write": write,
        }
    }


def _list(principal: str, repo: str | None = None) -> dict:
    request: dict = {"principal": principal}
    if repo is not None:
        request["repo"] = repo
    return {"request": request}


async def _scenario(stem: str) -> None:
    socket_path = os.environ["GRAPH_SERVICE_SOCKET"]
    admin = await MedullaClient.connect(
        socket_path=socket_path, verified_context=request_context(), timeout=30
    )
    reader = await MedullaClient.connect(
        socket_path=socket_path,
        verified_context=request_context(
            agent_id=READER, scopes=["graph:read"], roles=["test"]
        ),
        timeout=30,
    )
    one, two = f"grants-a-{stem}", f"grants-b-{stem}"
    try:
        # Nothing is granted yet: the list is empty, not an error.
        empty = await admin._send("ListPathGrants", _list(READER))
        assert empty["grants"] == []

        await admin._send("SetPathGrant", _grant(one, "src", True, True))
        await admin._send("SetPathGrant", _grant(one, "docs", True, False))
        await admin._send("SetPathGrant", _grant(two, "", True, False))

        everything = await reader._send("ListPathGrants", _list(READER))
        assert sorted((g["repo"], g["prefix"]) for g in everything["grants"]) == [
            (one, "docs"),
            (one, "src"),
            (two, ""),
        ]
        assert all(g["principal"] == READER for g in everything["grants"])
        narrowed = await reader._send("ListPathGrants", _list(READER, one))
        assert {g["prefix"] for g in narrowed["grants"]} == {"src", "docs"}
        none = await reader._send("ListPathGrants", _list(READER, f"absent-{stem}"))
        assert none["grants"] == []

        # A principal nobody granted reads empty for the administrator.
        assert (await admin._send("ListPathGrants", _list(f"nobody-{stem}")))[
            "grants"
        ] == []
        # The reader cannot read another principal's grants, existing or not.
        for other in (request_context()["agent_id"], f"nobody-{stem}"):
            with pytest.raises(Exception, match="ACCESS_DENIED"):
                await reader._send("ListPathGrants", _list(str(other)))

        # Both bits false is an explicit deny: it replaces the row and still lists.
        await admin._send("SetPathGrant", _grant(one, "docs", False, False))
        after = await reader._send("ListPathGrants", _list(READER, one))
        by_prefix = {g["prefix"]: g for g in after["grants"]}
        assert sorted(by_prefix) == ["docs", "src"]
        assert (by_prefix["docs"]["read"], by_prefix["docs"]["write"]) == (False, False)
        assert (by_prefix["src"]["read"], by_prefix["src"]["write"]) == (True, True)

        with pytest.raises(Exception, match="PATH_GRANT_INVALID"):
            await admin._send("ListPathGrants", _list(READER, "../x"))
    finally:
        await reader.close()
        await admin.close()


# spec: MEDULLA-SCM-R007.1
@pytest.mark.spec("MEDULLA-SCM-R007.1")
def test_list_path_grants_reads_own_grants_and_refuses_another_principals():
    asyncio.run(_scenario(uuid.uuid4().hex[:8]))
