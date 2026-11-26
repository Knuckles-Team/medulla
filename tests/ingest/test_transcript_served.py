"""Served transcript reads through `TranscriptRead`: MEDULLA-INGEST-WORKER-R003.17.1
and R003.18.

A stored thread is read back by its source identity under the verified tenant. The
stored policy
action `source:transcript:read` is held exactly: a same-tenant caller with a graph
read grant but
without it is refused and sees no text, and another tenant sees nothing.
"""

from __future__ import annotations

import os
import uuid
from typing import Any

import pytest
from conftest import request_context, tenant_request_context

from medulla.client import EngineResponseError, MedullaClient
from medulla.generated.ingestion import decode_transcript_read, send_transcript_read
from medulla.generated.source_ingestion import SourceIngestionDisposition

from .test_source_batch_kind_served import _transcript_batch
from .test_source_stream_versions_served import (
    STREAM,
    _close,
    _connector,
    _ingest_client,
    _submit,
)

READ_SCOPES = ["kg:read", "source:transcript:read"]


async def _connect(graph: str, context: dict[str, object]) -> MedullaClient:
    return await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=context,
        graph_name=graph,
    )


async def _read(client: MedullaClient, connector: str, **extra: Any) -> Any:
    params = {
        "connector": connector,
        "stream": STREAM,
        "session_id": "session-1",
        **extra,
    }
    return decode_transcript_read(await send_transcript_read(client, params))


@pytest.mark.spec("MEDULLA-INGEST-WORKER-R003.17.1")
@pytest.mark.asyncio
async def test_same_tenant_unauthorized_reads_are_denied() -> None:
    connector = _connector()
    concept = f"concept-{uuid.uuid4().hex[:8]}"
    client, graph = await _ingest_client(connector)
    holder: MedullaClient | None = None
    outsider: MedullaClient | None = None
    try:
        await client.nodes.add(concept, {"type": "Concept"})
        receipt = await _submit(
            client, _transcript_batch(connector, 1, None, concept, "claude"), "denied"
        )
        assert receipt.transcript is not None
        assert receipt.transcript.message_count == 2

        holder = await _connect(graph, request_context(scopes=READ_SCOPES))
        view = await _read(holder, connector)
        assert view.thread is not None
        texts = [message.text for message in view.thread.messages]
        assert texts == ["text of m1", "text of m2"]

        outsider = await _connect(graph, request_context(scopes=["kg:read"]))
        with pytest.raises(EngineResponseError) as denied:
            await _read(outsider, connector)
        assert "ACCESS_DENIED" in str(denied.value)
        assert "text of m1" not in str(denied.value)
    finally:
        for reader in (holder, outsider):
            if reader is not None:
                await reader.close()
        await _close(client, graph)


# proves R003.18: Hot-path proof: transcripts through the wire method
@pytest.mark.spec("MEDULLA-INGEST-WORKER-R003.18")
@pytest.mark.asyncio
async def test_thread_messages_are_shared_within_scope() -> None:
    """Identical session ids across two connectors stay distinct (the other connector
    names no
    stored thread), replay is idempotent, a second authorized agent reads timestamped
    messages
    and concept links, and another tenant reads nothing."""
    first, second = _connector(), _connector()
    concept = f"concept-{uuid.uuid4().hex[:8]}"
    client, graph = await _ingest_client(first)
    reader: MedullaClient | None = None
    foreign: MedullaClient | None = None
    try:
        await client.nodes.add(concept, {"type": "Concept"})
        batch = _transcript_batch(first, 1, None, concept, "claude")
        accepted = await _submit(client, batch, "first")
        assert accepted.disposition is SourceIngestionDisposition.COMMITTED
        again = await _submit(client, batch, "replay")
        assert again.disposition is SourceIngestionDisposition.REPLAYED

        reader = await _connect(graph, request_context(scopes=READ_SCOPES))
        view = await _read(reader, first)
        assert view.thread is not None
        assert view.thread.message_count == 2
        head, tail = view.thread.messages
        assert (head.sequence, tail.sequence) == (0, 1)
        assert head.event_time_ms == 1000 and head.storage_time_ms == 1005
        assert tail.event_time_ms == 2000
        assert head.concept_refs == [concept] and tail.concept_refs == []
        assert head.source_record_id == "record-m1"
        # A page: one message after sequence 0.
        page = await _read(reader, first, after_sequence=0, limit=1)
        assert page.thread is not None
        assert [m.sequence for m in page.thread.messages] == [1]
        assert page.thread.next_after_sequence is None
        # The other connector's identity names no stored thread.
        other = await _read(reader, second)
        assert other.thread is None

        foreign = await _connect(
            graph, tenant_request_context("tenant:foreign", scopes=READ_SCOPES)
        )
        try:
            seen = await _read(foreign, first)
            assert seen.thread is None
        except EngineResponseError:
            pass  # a refused cross-tenant read also sees nothing
    finally:
        for other_client in (reader, foreign):
            if other_client is not None:
                await other_client.close()
        await _close(client, graph)
