"""MEDULLA-SCM-R007: path-scoped access control of the source methods.

One principal holds read and write on ``src`` and a narrower grant on
``src/secret``. ``ReadTree`` drops the entries the grants deny, a read of a
denied path refuses ``PATH_READ_DENIED``, a write outside every grant refuses
``PATH_WRITE_DENIED`` and a grant with a ``..`` prefix refuses
``PATH_GRANT_INVALID``.
"""

from __future__ import annotations

import asyncio
import os
import uuid

import pytest
from conftest import TEST_AGENT_ID, TEST_TENANT, request_context

from medulla.client import MedullaClient

MODE = 0o100644


def _grant(graph: str, prefix: str, read: bool, write: bool) -> dict:
    return {
        "request": {
            "repo": graph,
            "principal": TEST_AGENT_ID,
            "prefix": prefix,
            "read": read,
            "write": write,
        }
    }


def _entry(path: str, blob: tuple[str, int]) -> dict:
    return {
        "path": path,
        "op": "add",
        "blob": {"digest": blob[0], "size": blob[1]},
        "mode": MODE,
        "renamed_from": None,
    }


def _submit(graph: str, branch: str, base: str | None, paths: list[dict]) -> dict:
    return {
        "request": {
            "repo": graph,
            "branch": branch,
            "base_head": base,
            "message": "add files\n\nSpec: MEDULLA-SCM-R007",
            "paths": paths,
            "trailers": None,
            "run_trace_id": None,
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
        branch = f"feature/access-{stem}"
        await client._send(
            "CreateBranch", {"tenant": TEST_TENANT, "graph": graph, "name": branch}
        )

        async def blob(text: str) -> tuple[str, int]:
            data = f"{text} {stem}\n".encode()
            return (await client.blob.store(data), len(data))

        # Default deny: with no grant a write refuses.
        with pytest.raises(Exception, match="PATH_WRITE_DENIED"):
            await client._send(
                "SubmitChangeset",
                _submit(graph, branch, None, [_entry("src/a", await blob("a"))]),
                idempotency_key=f"deny-{stem}",
            )

        # `src` is read and write; `src/secret` may be written but not read.
        await client._send("SetPathGrant", _grant(graph, "src", True, True))
        await client._send("SetPathGrant", _grant(graph, "src/secret", False, True))

        receipt = await client._send(
            "SubmitChangeset",
            _submit(
                graph,
                branch,
                None,
                [
                    _entry("src/a", await blob("a")),
                    _entry("src/pub/b", await blob("b")),
                    _entry("src/secret/a", await blob("s")),
                ],
            ),
            idempotency_key=f"land-{stem}",
        )
        head = receipt["branch_head"]

        def read(prefix: str) -> dict:
            return {
                "request": {
                    "repo": graph,
                    "branch": branch,
                    "rev": head,
                    "prefix": prefix,
                    "depth": 0,
                }
            }

        def path_read(method: str, path: str) -> tuple[str, dict]:
            return method, {
                "request": {
                    "repo": graph,
                    "branch": branch,
                    "rev": head,
                    "path": path,
                }
            }

        # The tree read drops the entries the grants deny.
        tree = await client._send("ReadTree", read(""))
        assert [e["path"] for e in tree["entries"]] == ["src/a", "src/pub/b"]
        with pytest.raises(Exception, match="PATH_READ_DENIED"):
            await client._send("ReadTree", read("src/secret"))

        # A read of a denied path refuses; a readable path answers.
        for method in ("ReadBlame", "ReadPathHistory"):
            name, params = path_read(method, "src/secret/a")
            with pytest.raises(Exception, match="PATH_READ_DENIED"):
                await client._send(name, params)
        name, params = path_read("ReadBlame", "src/a")
        assert (await client._send(name, params))["runs"]

        # A write outside every grant refuses and changes nothing.
        with pytest.raises(Exception, match="PATH_WRITE_DENIED"):
            await client._send(
                "SubmitChangeset",
                _submit(graph, branch, head, [_entry("docs/x", await blob("x"))]),
                idempotency_key=f"docs-{stem}",
            )

        # A grant that escapes the repository is invalid.
        with pytest.raises(Exception, match="PATH_GRANT_INVALID"):
            await client._send("SetPathGrant", _grant(graph, "../x", True, True))
        await client.tenants.delete(graph)
    finally:
        await client.close()


# spec: MEDULLA-SCM-R007
@pytest.mark.spec("MEDULLA-SCM-R007")
def test_prefix_grant_filters_reads_and_refuses_writes():
    asyncio.run(_scenario(f"scm-access-{uuid.uuid4().hex[:8]}"))
