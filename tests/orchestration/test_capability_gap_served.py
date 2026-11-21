"""Served capability gap writes (MEDULLA-ORCH-VOCAB-R004.6.2).

The entry point is the served ``AddNode``. A gap whose capability, task class and
domain exist is admitted; the same record again is an idempotent replay; the next
revision updates it; a stale revision refuses ``GAP_REVISION_CONFLICT``; a foreign
owner refuses ``GAP_OWNER_DENIED``; and a reference the graph does not hold refuses
``GAP_REFERENCE_UNKNOWN`` and creates nothing.
"""

from __future__ import annotations

import os
import uuid
from typing import Any

import pytest
from conftest import request_context

from medulla.client import MedullaClient


async def _connect() -> MedullaClient:
    return await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
    )


def _gap(tag: str, revision: int, owner: str = "principal:a", **overrides: Any) -> dict:
    props: dict[str, Any] = {
        "_capability_gap": True,
        "tenant_id": "tenant-a",
        "node_id": f"gap:{tag}",
        "revision": revision,
        "capability_ref": f"capability:{tag}",
        "task_class_ref": f"task-class:{tag}",
        "domain_ref": f"domain:{tag}",
        "source_evidence_digest": "sha256:" + "ab" * 32,
        "owner_principal": owner,
        "disposition": "open",
    }
    props.update(overrides)
    return props


@pytest.mark.spec("MEDULLA-ORCH-VOCAB-R004.6.2")
@pytest.mark.asyncio
async def test_dedup_conflict_and_invalid_taxonomy():
    tag = uuid.uuid4().hex[:8]
    client = await _connect()
    try:
        for ref in ("capability", "task-class", "domain"):
            await client.nodes.add(f"{ref}:{tag}", {"type": "Taxonomy"})
        node = f"gap:{tag}"

        # Admitted once; the same source event again is idempotent.
        await client.nodes.add(node, _gap(tag, 1))
        await client.nodes.add(node, _gap(tag, 1))

        # The next revision updates the gap; a stale one refuses.
        await client.nodes.add(node, _gap(tag, 2, disposition="accepted"))
        with pytest.raises(Exception, match="GAP_REVISION_CONFLICT"):
            await client.nodes.add(node, _gap(tag, 2, disposition="rejected"))
        with pytest.raises(Exception, match="GAP_REVISION_CONFLICT"):
            await client.nodes.add(node, _gap(tag, 1))

        # A foreign owner cannot rewrite it.
        with pytest.raises(Exception, match="GAP_OWNER_DENIED"):
            await client.nodes.add(node, _gap(tag, 3, owner="principal:b"))

        # An unknown taxonomy reference refuses and creates no term.
        other = f"gap:{tag}:other"
        with pytest.raises(Exception, match="GAP_REFERENCE_UNKNOWN"):
            await client.nodes.add(
                other,
                _gap(tag, 1, node_id=other, capability_ref=f"capability:missing-{tag}"),
            )
        assert not await client.nodes.has(other)
    finally:
        await client.close()
