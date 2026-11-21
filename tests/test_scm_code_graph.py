"""MEDULLA-SCM-R006: the code graph stays live with changesets.

Three Python files land in one changeset. A one-file structural changeset
replaces only that file's nodes and leaves the other two files' nodes and the
community of their call alike; a whitespace change writes only the generation
node; a changeset that does not chain from the stored generation returns the
receipt ``index`` ``Refused`` with ``INDEX_BASE_MISMATCH``.
"""

from __future__ import annotations

import asyncio
import os
import uuid

import pytest
from conftest import TEST_AGENT_ID, TEST_TENANT, request_context

from medulla.client import MedullaClient

MODE = 0o100644

FILES = {
    "a.py": "def alpha():\n    return 1\n",
    "b.py": "def beta():\n    return gamma()\n",
    "c.py": "def gamma():\n    return 2\n",
}


def _entry(path: str, op: str, blob: tuple[str, int] | None) -> dict:
    return {
        "path": path,
        "op": op,
        "blob": None if blob is None else {"digest": blob[0], "size": blob[1]},
        "mode": MODE,
        "renamed_from": None,
    }


def _submit(graph: str, branch: str, base: str | None, n: int, paths: list[dict]) -> dict:
    return {
        "request": {
            "repo": graph,
            "branch": branch,
            "base_head": base,
            "message": f"change {n}\n\nSpec: MEDULLA-SCM-R006",
            "paths": paths,
            "trailers": None,
            "run_trace_id": None,
        }
    }


async def _symbols_by_file(client: MedullaClient) -> dict[str, set[str]]:
    by_file: dict[str, set[str]] = {}
    for node_id, _ in await client.nodes.list_by_label("SYMBOL", 0):
        properties = await client.nodes.properties(node_id) or {}
        by_file.setdefault(properties["file_path"], set()).add(node_id)
    return by_file


async def _features(client: MedullaClient) -> set[str]:
    return {node_id for node_id, _ in await client.nodes.list_by_label("CodeFeature", 0)}


async def _scenario(graph: str) -> None:
    client = await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
        timeout=60,
    )
    try:
        await client.tenants.create(graph, "Global")
        stem = uuid.uuid4().hex[:8]
        branch = f"feature/code-graph-{stem}"
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

        async def blob(text: str) -> tuple[str, int]:
            data = text.encode()
            return (await client.blob.store(data), len(data))

        async def land(n: int, head: str | None, paths: list[dict]) -> dict:
            return await client._send(
                "SubmitChangeset",
                _submit(graph, branch, head, n, paths),
                idempotency_key=f"cg{n}-{stem}",
            )

        first = await land(
            0,
            None,
            [_entry(path, "add", await blob(text)) for path, text in FILES.items()],
        )
        assert first["index"]["outcome"] == "indexed", first["index"]
        assert first["index"]["reparsed"] == 3
        before = await _symbols_by_file(client)
        assert set(before) == set(FILES)
        features_before = await _features(client)
        assert features_before, "beta calls gamma, so they share a community"

        # A structural change to one file replaces that file's symbols only.
        second = await land(
            1,
            first["branch_head"],
            [_entry("a.py", "modify", await blob("def delta():\n    return 1\n"))],
        )
        assert second["index"]["outcome"] == "indexed", second["index"]
        assert second["index"]["reparsed"] == 1
        after = await _symbols_by_file(client)
        assert after["a.py"] != before["a.py"]
        assert after["b.py"] == before["b.py"]
        assert after["c.py"] == before["c.py"]
        assert await _features(client) == features_before

        # A whitespace change updates nothing but the generation node.
        symbols = await _symbols_by_file(client)
        third = await land(
            2,
            second["branch_head"],
            [_entry("b.py", "modify", await blob("def beta():\n\n    return gamma()\n"))],
        )
        assert third["index"] == {
            "outcome": "indexed",
            "reparsed": 0,
            "deleted": 0,
            "communities": 0,
        }
        assert await _symbols_by_file(client) == symbols
        generation = await client.nodes.properties(f"codegen:{graph}")
        assert generation["map_generation"] == "3"
        assert generation["base_revision"] == third["branch_head"]

        # A changeset whose parent is not the stored base is accepted but not indexed.
        await client.nodes.remove(f"codegen:{graph}")
        await client.nodes.add(
            f"codegen:{graph}",
            {**generation, "type": "RepositoryGeneration", "base_revision": "stale"},
        )
        fourth = await land(
            3,
            third["branch_head"],
            [_entry("c.py", "modify", await blob("def gamma():\n    return 3\n"))],
        )
        assert fourth["index"] == {"outcome": "refused", "code": "INDEX_BASE_MISMATCH"}
        assert await _symbols_by_file(client) == symbols
        await client.tenants.delete(graph)
    finally:
        await client.close()


# spec: MEDULLA-SCM-R006
@pytest.mark.spec("MEDULLA-SCM-R006")
def test_one_file_change_updates_only_its_symbols():
    asyncio.run(_scenario(f"scm-code-graph-{uuid.uuid4().hex[:8]}"))
