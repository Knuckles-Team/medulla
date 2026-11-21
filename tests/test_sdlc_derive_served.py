"""MEDULLA-SDLC-R001.3 and R001.6: the served derivation of lifecycle edges.

A submitted changeset writes its `commit:<id>` node and its derived edges in one batch into
the repository graph; a scoped `IndexRepository` writes `changeset_id` and `seq` on the
`commit:` node of each ingested git commit; a 50-path changeset derives the expected edge
count.
"""

from __future__ import annotations

import asyncio
import hashlib
import os
import uuid

import msgpack
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


def _request(graph: str, branch: str, base: str | None, message: str, paths: list[dict]) -> dict:
    return {
        "repo": graph,
        "branch": branch,
        "base_head": base,
        "message": message,
        "paths": paths,
        "trailers": None,
        "run_trace_id": None,
    }


def _body(raw):
    return msgpack.unpackb(raw, raw=False) if isinstance(raw, bytes) else raw


async def _connect(graph: str) -> tuple[MedullaClient, str]:
    client = await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
        timeout=60,
    )
    await client.tenants.create(graph, "Global")
    stem = uuid.uuid4().hex[:8]
    branch = f"feature/derive-{stem}"
    await client._send("CreateBranch", {"tenant": TEST_TENANT, "graph": graph, "name": branch})
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
    return client, branch


async def _put_node(client: MedullaClient, graph: str, node_id: str, props: dict) -> None:
    await client._send(
        "AddNode",
        {"node_id": node_id, "properties_msgpack": msgpack.packb(props, use_bin_type=True)},
        graph,
    )


async def _submit(client, graph, branch, base, message, files: dict[str, bytes], key):
    entries = []
    for path, data in files.items():
        digest = await client.blob.store(data)
        entries.append(_entry(path, "add", (digest, len(data))))
    return await client._send(
        "SubmitChangeset",
        {"request": _request(graph, branch, base, message, entries)},
        idempotency_key=key,
    )


@pytest.mark.spec("MEDULLA-SDLC-R001.3")
def test_accept_writes_derived_edges(start_medulla_server) -> None:
    graph = f"derive-{uuid.uuid4().hex[:8]}"

    async def scenario() -> None:
        client, branch = await _connect(graph)
        try:
            requirement = f"requirement:{graph}:DEMO-R001"
            await _put_node(client, graph, requirement, {"type": "Requirement", "text": "x"})
            first = await _submit(
                client, graph, branch, None, "add\n\nSpec: DEMO-R001",
                {"a.py": b"def alpha():\n    return 1\n"}, "d1",
            )
            node_id = f"commit:{first['changeset']['id']}"
            node = _body(await client._send("GetNodeProperties", {"node_id": node_id}, graph))
            assert node["type"] == "Changeset"
            assert node["changeset_id"] == first["changeset"]["id"]
            assert str(node["seq"]) == "1"
            assert node["repo"] == graph
            # spec: MEDULLA-SDLC-R001.3
            assert requirement in await client.successors(node_id)
            # A replay under the same key derives again and changes nothing.
            again = await client._send(
                "SubmitChangeset",
                {"request": _request(graph, branch, None, "add\n\nSpec: DEMO-R001", [
                    _entry("a.py", "add", (await client.blob.store(b"def alpha():\n    return 1\n"), 25))
                ])},
                idempotency_key="d1",
            )
            assert again["replayed"] is True
            assert requirement in await client.successors(node_id)
        finally:
            await client.close()

    asyncio.run(scenario())


@pytest.mark.spec("MEDULLA-SDLC-R001.3")
def test_index_repository_derives_edges_for_git_commits(clean_graph) -> None:
    from medulla.generated.index_repository import IndexRepositoryScope

    repo = f"repo-derive-{uuid.uuid4().hex[:8]}"
    first, second = "a" * 40, "b" * 40
    body = b"def alpha():\n    return 1\n"
    scope = IndexRepositoryScope.model_validate(
        {
            "repository_id": repo,
            "commits": [
                {"digest": first, "author": "alice", "timestamp_ms": 1, "parents": [], "changes": []},
                {"digest": second, "author": "alice", "timestamp_ms": 2, "parents": [first], "changes": []},
            ],
            "refs": [{"ref_name": "refs/heads/main", "revision_id": second, "status": "live"}],
            "file_versions": [
                {
                    "ref_name": "refs/heads/main",
                    "path": "a.py",
                    "blob_digest": "sha256:" + hashlib.sha256(body).hexdigest(),
                }
            ],
        }
    )
    try:
        clean_graph.graph.index_repository([("a.py", body)], scope=scope)
    except Exception as exc:
        if "AST feature not enabled" in str(exc) or "requires durable persistence" in str(exc):
            pytest.skip("engine built without the ast feature or durable persistence")
        raise
    # spec: MEDULLA-SDLC-R001.3
    seqs = {}
    for digest in (first, second):
        props = clean_graph.nodes.properties(f"commit:{digest}")
        assert props["changeset_id"] == digest
        seqs[digest] = int(props["seq"])
    assert seqs == {first: 1, second: 2}


@pytest.mark.spec("MEDULLA-SDLC-R001.6")
def test_fifty_path_changeset_derives_expected_edge_count(start_medulla_server) -> None:
    graph = f"derive50-{uuid.uuid4().hex[:8]}"

    async def scenario() -> None:
        client, branch = await _connect(graph)
        try:
            for n in range(3):
                await _put_node(
                    client, graph, f"requirement:{graph}:DEMO-R00{n}", {"type": "Requirement", "text": "x"}
                )
            files = {f"f{n}.py": f"def fn{n}():\n    return {n}\n".encode() for n in range(50)}
            receipt = await _submit(
                client, graph, branch, None,
                "fifty\n\nSpec: DEMO-R000, DEMO-R001, DEMO-R002", files, "d50",
            )
            node_id = f"commit:{receipt['changeset']['id']}"
            successors = await client.successors(node_id)
            # Each cited requirement is one `citesRequirement` edge, and each indexed
            # symbol whose span changed is one `changes` edge (a new file changes it all).
            cites = [s for s in successors if s.startswith("requirement:")]
            changes = [s for s in successors if not s.startswith("requirement:")]
            # spec: MEDULLA-SDLC-R001.6
            assert len(cites) == 3
            assert len(changes) == 50
        finally:
            await client.close()

    asyncio.run(scenario())
