"""Served `SourceIngest` batch kind: MEDULLA-INGEST-WORKER-R902.3.1."""

from __future__ import annotations

import uuid
from typing import Any

import pytest
from pydantic import ValidationError

from medulla.client import EngineResponseError
from medulla.generated.source_ingestion import (
    SourceBatchKind,
    SourceIngestionDisposition,
    SourceIngestionRequest,
)

from .test_source_stream_versions_served import (
    STREAM,
    _close,
    _connector,
    _ingest_client,
    _submit,
)

CHECKPOINT = {"stream": "items", "position": {"page": 1}}
BASE = {
    "connector": "demo-connector",
    "mode": "delta",
    "strict_schema": True,
    "records": [],
    "provider_checkpoint": CHECKPOINT,
}


@pytest.mark.spec("MEDULLA-INGEST-WORKER-R902.3.1")
def test_mismatched_kind_is_refused(clean_graph):
    # The kind/extension rule is enforced by the typed request decoder the
    # engine and the client share, so the request is refused before it is
    # sent: the client names it as an invalid engine request with the code.
    mismatched = {**BASE, "kind": "access"}
    with pytest.raises(
        ValueError,
        match="is not a valid engine request: .*SOURCE_BATCH_KIND_MISMATCH",
    ):
        clean_graph._send("SourceIngest", {"request": mismatched})


@pytest.mark.spec("MEDULLA-INGEST-WORKER-R902.3.1")
def test_client_model_carries_kind_into_the_digest():
    records = SourceIngestionRequest.model_validate(BASE)
    access = SourceIngestionRequest.model_validate(
        {**BASE, "kind": "access", "extension": {"access": []}}
    )
    assert records.kind is SourceBatchKind.RECORDS
    assert records.canonical_digest() != access.canonical_digest()
    with pytest.raises(ValidationError):
        SourceIngestionRequest.model_validate(
            {**BASE, "kind": "access", "extension": {"access": [], "groups": []}}
        )


def _transcript_batch(
    connector: str, page: int, previous: int | None, concept: str, dialect: str
) -> dict[str, Any]:
    """One thread of two messages; the first names `concept`."""

    def message(message_id: str, event: int, concepts: list[str]) -> dict[str, Any]:
        return {
            "message": {
                "connector": connector,
                "stream": STREAM,
                "session_id": "session-1",
                "message_id": message_id,
                "role": "user",
                "event_time_ms": event,
                "storage_time_ms": event + 5,
                "text": f"text of {message_id}",
                "concept_refs": concepts,
                "provenance": {
                    "source_record_id": f"record-{message_id}",
                    "source_uri": f"demo://session-1/{message_id}",
                },
            }
        }

    return {
        "connector": connector,
        "kind": "transcripts",
        "mode": "delta",
        "strict_schema": True,
        "records": [],
        "relationships": [],
        "withdrawals": [],
        "extension": {
            "transcripts": [
                {
                    "thread": {
                        "dialect": dialect,
                        "thread": {
                            "connector": connector,
                            "stream": STREAM,
                            "session_id": "session-1",
                            "message_ids": ["m1", "m2"],
                        },
                    }
                },
                message("m1", 1000, [concept]),
                message("m2", 2000, []),
            ]
        },
        "provider_checkpoint": {"stream": STREAM, "position": {"transcript": page}},
        "expected_previous_checkpoint": (
            None
            if previous is None
            else {"stream": STREAM, "position": {"transcript": previous}}
        ),
    }


@pytest.mark.spec("MEDULLA-INGEST-WORKER-R003.15")
@pytest.mark.asyncio
async def test_transcripts_batch_commits_thread_and_messages() -> None:
    connector = _connector()
    concept = f"concept-{uuid.uuid4().hex[:8]}"
    client, graph = await _ingest_client(connector)
    try:
        await client.nodes.add(concept, {"type": "Concept"})
        receipt = await _submit(
            client,
            _transcript_batch(connector, 1, None, concept, "claude"),
            "transcript",
        )
        assert receipt.disposition is SourceIngestionDisposition.COMMITTED
        threads = await client.nodes.list_by_label("Transcript", 0)
        assert len(threads) == 1
        thread_id = threads[0][0]
        thread = await client.nodes.properties(thread_id)
        assert thread["dialect"] == "claude"
        assert thread["message_count"] == 2

        segments = {}
        for node_id, _ in await client.nodes.list_by_label("TranscriptSegment", 0):
            props = await client.nodes.properties(node_id)
            segments[props["sequence"]] = (node_id, props)
        assert sorted(segments) == [0, 1]
        first_id, first = segments[0]
        second_id, second = segments[1]
        assert first["text"] == "text of m1"
        assert first["event_time"] == 1000
        assert first["storage_time"] == 1005
        assert first["source_record_id"] == "record-m1"
        assert "narrative_time" not in first
        assert second["event_time"] == 2000

        # Each message belongs to the thread, the first precedes the second and the
        # first mentions the concept.
        assert await client.edges.has(first_id, thread_id)
        assert await client.edges.has(second_id, thread_id)
        assert await client.edges.has(first_id, second_id)
        assert await client.edges.has(first_id, concept)
        assert not await client.edges.has(second_id, concept)

        # The same batch replays.
        again = await _submit(
            client,
            _transcript_batch(connector, 1, None, concept, "claude"),
            "again",
        )
        assert again.disposition is SourceIngestionDisposition.REPLAYED
    finally:
        await _close(client, graph)


@pytest.mark.spec("MEDULLA-INGEST-WORKER-R003.15")
@pytest.mark.asyncio
async def test_transcripts_refusals_write_nothing() -> None:
    connector = _connector()
    client, graph = await _ingest_client(connector)
    try:
        # A concept the graph does not hold, and a format that is not declared.
        with pytest.raises(EngineResponseError) as foreign:
            await _submit(
                client,
                _transcript_batch(connector, 1, None, "missing-concept", "claude"),
                "foreign",
            )
        assert "TRANSCRIPT_FOREIGN_CONCEPT" in str(foreign.value)
        with pytest.raises(EngineResponseError) as unknown:
            await _submit(
                client,
                _transcript_batch(connector, 1, None, "missing-concept", "plain-text"),
                "unknown",
            )
        assert "TRANSCRIPT_UNKNOWN_FORMAT" in str(unknown.value)
        assert await client.nodes.list_by_label("Transcript", 0) == []
        assert await client.nodes.list_by_label("TranscriptSegment", 0) == []
    finally:
        await _close(client, graph)
