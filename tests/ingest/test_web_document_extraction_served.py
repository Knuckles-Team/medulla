"""Served extraction jobs of an enriched Document: MEDULLA-UNIFIED-DATA-PLANE-R651.7.7.

`EnrichDocument` enqueues one `extraction` job per chunk on the `ingest` queue through the
one ingest enqueue path. A worker claims the jobs through `TaskQueueClaim`.
"""

from __future__ import annotations

import json
import uuid
from typing import Any

import pytest

from .test_ingest_queue_served import _claim, _client

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
