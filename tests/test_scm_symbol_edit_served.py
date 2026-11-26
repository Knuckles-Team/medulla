"""MEDULLA-SCM-R013.4 and MEDULLA-SCM-R013.5: the served ``SubmitSymbolEdit`` method.

One Python function is replaced by symbol: the edit lands one changeset, the file
bytes outside the symbol are unchanged, and the re-index shows the new body. A stale
anchor digest, an unknown symbol and an unparseable replacement each refuse with
their code and leave the branch head where it was.
"""

from __future__ import annotations

import asyncio
import hashlib
import os
import uuid

import pytest
from conftest import TEST_AGENT_ID, TEST_TENANT, request_context

from medulla.client import MedullaClient

MODE = 0o100644
NEW = "def alpha():\n    return 9"
SOURCE = "def alpha():\n    return 1\n\n\ndef beta():\n    return 2\n"


async def _scenario(graph: str) -> None:
    client = await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
        timeout=60,
    )
    try:
        await client.tenants.create(graph, "Global")
        stem = uuid.uuid4().hex[:8]
        branch = f"feature/edit-{stem}"
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
        data = SOURCE.encode()
        digest = await client.blob.store(data)
        landed = await client._send(
            "SubmitChangeset",
            {
                "request": {
                    "repo": graph,
                    "branch": branch,
                    "base_head": None,
                    "message": "seed\n\nSpec: MEDULLA-SCM-R013.4",
                    "paths": [
                        {
                            "path": "a.py",
                            "op": "add",
                            "blob": {"digest": digest, "size": len(data)},
                            "mode": MODE,
                            "renamed_from": None,
                        }
                    ],
                    "trailers": None,
                    "run_trace_id": None,
                }
            },
            idempotency_key=f"edit-seed-{stem}",
        )
        head = landed["branch_head"]
        symbols = {}
        for node_id, _ in await client.nodes.list_by_label("SYMBOL", 0):
            properties = await client.nodes.properties(node_id) or {}
            symbols[properties["name"]] = (node_id, properties)
        alpha_id, alpha = symbols["alpha"]
        start, end = int(alpha["start_byte"]), int(alpha["end_byte"])
        anchor = hashlib.sha256(data[start:end]).hexdigest()

        def edit(symbol_id: str, anchor_digest: str, new_text: str) -> dict:
            return {
                "request": {
                    "repo": graph,
                    "branch": branch,
                    "base_rev": head,
                    "ops": [
                        {
                            "kind": "replace_symbol",
                            "symbol_id": symbol_id,
                            "anchor_span_digest": anchor_digest,
                            "new_text": new_text,
                        }
                    ],
                    "message": "edit alpha\n\nSpec: MEDULLA-SCM-R013.4",
                    "run_trace_id": "",
                }
            }

        refusals = (
            ("EDIT_BASE_STALE", edit(alpha_id, "0" * 64, "def alpha():\n    return 9")),
            (
                "EDIT_SYMBOL_UNKNOWN",
                edit("sym:missing", anchor, "def alpha():\n    return 9"),
            ),
            ("EDIT_UNPARSEABLE", edit(alpha_id, anchor, "def alpha(:\n    return 9")),
        )
        for code, request in refusals:
            with pytest.raises(Exception, match=code):
                await client._send(
                    "SubmitSymbolEdit", request, idempotency_key=f"edit-{code}-{stem}"
                )
        history = await client._send(
            "ReadPathHistory",
            {
                "request": {
                    "repo": graph,
                    "branch": branch,
                    "rev": head,
                    "path": "a.py",
                    "limit": 0,
                }
            },
        )
        assert len(history["entries"]) == 1, "a refusal landed a changeset"

        done = await client._send(
            "SubmitSymbolEdit",
            edit(alpha_id, anchor, "def alpha():\n    return 42"),
            idempotency_key=f"edit-ok-{stem}",
        )
        assert done["replayed"] is False and done["branch_head"] != head
        tree = await client._send(
            "ReadTree",
            {
                "request": {
                    "repo": graph,
                    "branch": branch,
                    "rev": done["branch_head"],
                    "prefix": "a.py",
                    "depth": 0,
                }
            },
        )
        entry = tree["entries"][0]
        assert entry["blob_digest"] != digest
        new_bytes = await client.blob.fetch(entry["blob_digest"])
        assert (
            new_bytes
            == (SOURCE[:start] + "def alpha():\n    return 42" + SOURCE[end:]).encode()
        )
        await client.tenants.delete(graph)
    finally:
        await client.close()


# spec: MEDULLA-SCM-R013.4 MEDULLA-SCM-R013.5
@pytest.mark.spec("MEDULLA-SCM-R013.4")
@pytest.mark.spec("MEDULLA-SCM-R013.5")
def test_symbol_edit_lands_and_reindexes():
    asyncio.run(_scenario(f"scm-edit-{uuid.uuid4().hex[:8]}"))
