"""MEDULLA-DURABLE-KERNEL-R031.3: ``AuditAppend`` evidence through the served engine."""

from __future__ import annotations

import asyncio
import os
import uuid
from typing import Any

import pytest
from conftest import request_context

from medulla.client import MedullaClient

_DIGEST = "ab" * 32
_REFUSED = "INVALID_ARGUMENT|AUDIT_FIELDS_INVALID|invalid"


async def _append(client: MedullaClient, graph: str, request_id: str, **extra: Any):
    return await client.admin.audit_append(
        op="screen.verdict",
        surface="http",
        params_sha256=_DIGEST,
        status="reserved",
        request_id=request_id,
        audit_class="event",
        **extra,
    )


async def _scenario(graph: str) -> None:
    client = await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
        timeout=30,
    )
    try:
        await client.tenants.create(graph, "Global")
        receipt = await _append(
            client,
            graph,
            "req-evidence",
            evidence={"screen_verdict": "pass", "redaction_count": "2"},
        )
        assert receipt["replayed"] is False
        # The same request replays; the same identity with other evidence is a conflict.
        again = await _append(
            client,
            graph,
            "req-evidence",
            evidence={"screen_verdict": "pass", "redaction_count": "2"},
        )
        assert again["replayed"] is True
        with pytest.raises(Exception, match="CONFLICT|IDEMPOTENCY"):
            await _append(
                client, graph, "req-evidence", evidence={"screen_verdict": "fail"}
            )
        # An oversize map, or a value carrying a chain separator, writes nothing.
        with pytest.raises(Exception, match=_REFUSED):
            await _append(
                client, graph, "req-big", evidence={f"k{n}": "v" for n in range(17)}
            )
        with pytest.raises(Exception, match=_REFUSED):
            await _append(client, graph, "req-sep", evidence={"k": "a|b"})
        await client.tenants.delete(graph)
    finally:
        await client.close()


# spec: MEDULLA-DURABLE-KERNEL-R031.3
@pytest.mark.spec("MEDULLA-DURABLE-KERNEL-R031.3")
def test_evidence_reaches_the_chain_entry_and_oversize_is_refused():
    asyncio.run(_scenario(f"audit-evidence-{uuid.uuid4().hex[:8]}"))
