"""MEDULLA-DECISION-ENGINE-R617.6 served chat log to memory.

A transcripts batch committed through ``SourceIngest`` becomes episodic memory
of the same graph: the turns recall through ``MemoryRecall`` under a token
budget, keyed by the conversation id, and the same batch replays without
writing new records.
"""

from __future__ import annotations

import sys
import uuid
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from ingest.test_source_batch_kind_served import _transcript_batch
from ingest.test_source_stream_versions_served import (
    _close,
    _connector,
    _ingest_client,
    _submit,
)

from medulla import decision_client as dc
from medulla.generated.source_ingestion import SourceIngestionDisposition


# spec: MEDULLA-DECISION-ENGINE-R617.6
@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R617.6")
@pytest.mark.asyncio
async def test_chat_log_becomes_recallable_memory() -> None:
    connector = _connector()
    concept = f"concept-{uuid.uuid4().hex[:8]}"
    client, graph = await _ingest_client(connector)
    try:
        await client.nodes.add(concept, {"type": "Concept"})
        decision = dc.DecisionClient(client=client, graph=graph)
        receipt = await _submit(
            client,
            _transcript_batch(connector, 1, None, concept, "claude"),
            "transcript",
        )
        assert receipt.disposition is SourceIngestionDisposition.COMMITTED

        recalled = await decision.memory_recall({"keywords": ["text"], "budget": 1_000})
        records = {
            hit["record"]["memory_id"]: hit["record"] for hit in recalled["hits"]
        }
        assert set(records) == {"session-1:m1", "session-1:m2"}
        for record in records.values():
            assert record["tier"] == "episodic"
            assert record["session_id"] == "session-1"
            assert record["provenance_key"].startswith("record-")
        assert recalled["tokens_used"] <= 1_000
        # A budget below one record's size recalls none.
        tight = await decision.memory_recall({"keywords": ["text"], "budget": 1})
        assert tight["hits"] == []

        # The same batch replays and the memory is unchanged.
        again = await _submit(
            client,
            _transcript_batch(connector, 1, None, concept, "claude"),
            "again",
        )
        assert again.disposition is SourceIngestionDisposition.REPLAYED
        replayed = await decision.memory_recall({"keywords": ["text"], "budget": 1_000})
        assert {hit["record"]["memory_id"] for hit in replayed["hits"]} == set(records)
    finally:
        await _close(client, graph)
