"""Served extraction jobs of an enriched Document: MEDULLA-UNIFIED-DATA-PLANE-R651.7.7.

`EnrichDocument` enqueues one `extraction` job per chunk on the `ingest` queue through the
one ingest enqueue path. A worker claims the jobs through `TaskQueueClaim`.
"""

from __future__ import annotations

import hashlib
import json
import uuid
from typing import Any

import pytest

from .test_fact_grounding_served import _batch, _connector, _fact, _grounded_client
from .test_ingest_queue_served import _claim, _client
from .test_source_stream_versions_served import _submit

PAGE = "# Extraction\n\n" + "".join(
    f"## Section {section}\n\n"
    + "Ada Lovelace wrote the first program for the Analytical Engine. " * 12
    + "\n\n"
    for section in range(6)
)


async def _claim_extraction(client: Any, worker: str, document_id: str) -> list[dict[str, Any]]:
    """Every claimable extraction task of `document_id` (the queue is shared by the tenant)."""
    claimed: list[dict[str, Any]] = []
    while (task := await _claim(client, worker)) is not None:
        envelope = json.loads(task["payload_ref"])
        if envelope["corpus_key"] == document_id:
            claimed.append({"task": task, "envelope": envelope})
    return claimed


@pytest.mark.spec("MEDULLA-UNIFIED-DATA-PLANE-R651.7.7")
@pytest.mark.asyncio
async def test_enriched_chunks_extract_facts() -> None:
    client = await _client()
    try:
        document_id = f"doc:extraction-{uuid.uuid4().hex[:8]}"
        await client.nodes.add(
            document_id,
            {
                "type": "Document",
                "title": "Extraction",
                "text": PAGE,
                "url": "https://example.invalid/extraction",
            },
        )
        request = {"document_id": document_id, "max_chunk_bytes": 600, "overlap_bytes": 80}
        receipt = await client._send("EnrichDocument", {"request": request})
        chunk_ids = receipt["chunk_ids"]
        assert len(chunk_ids) > 1
        assert not receipt.get("warnings")
        digest = receipt["content_digest"].removeprefix("sha256:")

        claimed = await _claim_extraction(client, "extraction-worker-1", document_id)
        assert len(claimed) == len(chunk_ids), "one extraction job per chunk"
        by_ordinal = sorted(
            claimed, key=lambda item: int(item["envelope"]["job_id"].rsplit(":", 1)[1])
        )
        for ordinal, item in enumerate(by_ordinal):
            task, envelope = item["task"], item["envelope"]
            assert task["queue"] == "ingest"
            assert task["kind"] == "extraction"
            assert envelope["job_id"] == f"{document_id}:{digest}:{ordinal}"
            assert envelope["kind"] == "extraction"
            assert envelope["target"] == chunk_ids[ordinal]
            assert envelope["attempt"] == 0

        # A repeat of the same enrich enqueues nothing new.
        again = await client._send("EnrichDocument", {"request": request})
        assert again["replayed"] is True
        assert await _claim_extraction(client, "extraction-worker-1", document_id) == []
    finally:
        await client.close()


@pytest.mark.spec("MEDULLA-UNIFIED-DATA-PLANE-R651.7.7")
@pytest.mark.asyncio
async def test_extracted_facts_are_grounded_to_their_chunk_and_document() -> None:
    connector = _connector()
    client, _graph = await _grounded_client(connector)
    try:
        document_id = f"doc:grounding-{uuid.uuid4().hex[:8]}"
        await client.nodes.add(
            document_id,
            {
                "type": "Document",
                "title": "Grounding",
                "text": PAGE,
                "url": "https://example.invalid/grounding",
            },
        )
        receipt = await client._send(
            "EnrichDocument",
            {"request": {"document_id": document_id, "max_chunk_bytes": 600, "overlap_bytes": 80}},
        )
        chunk_id = receipt["chunk_ids"][0]

        def facts_batch(page: int, facts: list[dict[str, Any]]) -> dict[str, Any]:
            return {**_batch(connector, page, facts), "source_chunk": chunk_id}

        first = await _submit(
            client,
            facts_batch(
                1,
                [
                    _fact("Ada", "wrote", "Note G", 10, 30),
                    _fact("Ada", "annotated", "Note G", 40, 60),
                ],
            ),
            "first",
        )
        assert first.facts.created == 2
        assert len(first.facts.chunks) == 1
        outcome = first.facts.chunks[0]
        assert outcome.chunk_id == chunk_id
        assert (outcome.created, outcome.unchanged, outcome.rejected, outcome.superseded) == (
            2,
            0,
            0,
            0,
        )
        chunk = await client.nodes.properties(chunk_id)
        assert chunk is not None
        assert outcome.content_hash == hashlib.sha256(chunk["text"].encode()).hexdigest()

        # Each fact links to its chunk and to the Document.
        wrote, annotated = (entry.claim_id for entry in first.facts.entries)
        for claim_id in (wrote, annotated):
            to_chunk = await client.edges.properties(claim_id, chunk_id)
            assert to_chunk is not None and to_chunk["relationship"] == "extractedFrom"
            to_document = await client.edges.properties(claim_id, document_id)
            assert to_document is not None and to_document["relationship"] == "derivedFrom"

        # A re-extraction of the same chunk that no longer states one fact supersedes it.
        second = await _submit(
            client,
            facts_batch(2, [_fact("Ada", "wrote", "Note G", 10, 30)]),
            "second",
        )
        assert second.facts.unchanged == 1
        assert second.facts.chunks[0].superseded == 1
        assert await client.nodes.properties(annotated) is None
        assert await client.nodes.properties(wrote) is not None
    finally:
        await client.close()
