"""Served `SourceIngest` facts batches: MEDULLA-INGEST-WORKER-R701.5, R701.7."""

from __future__ import annotations

from typing import Any

import pytest

from medulla.generated.source_ingestion import (
    FactOutcome,
    SourceIngestionDisposition,
)

from .test_source_stream_versions_served import (
    STREAM,
    _close,
    _connector,
    _ingest_client,
    _submit,
)


def _fact(
    subject: str, predicate: str, obj: str, start: int, end: int
) -> dict[str, Any]:
    return {
        "subject": subject,
        "predicate": predicate,
        "object": obj,
        "evidence": {"source_ref": "doc:notes", "start": start, "end": end},
        "confidence": 0.5,
    }


def _batch(connector: str, page: int, facts: list[dict[str, Any]]) -> dict[str, Any]:
    return {
        "connector": connector,
        "kind": "facts",
        "mode": "delta",
        "strict_schema": True,
        "records": [],
        "relationships": [],
        "withdrawals": [],
        "extension": {"facts": facts},
        "provider_checkpoint": {"stream": STREAM, "position": {"facts": page}},
        "expected_previous_checkpoint": (
            None
            if page == 1
            else {"stream": STREAM, "position": {"facts": page - 1}}
        ),
    }


@pytest.mark.spec("MEDULLA-INGEST-WORKER-R701.5")
@pytest.mark.asyncio
async def test_unchanged_content_is_unchanged() -> None:
    connector = _connector()
    client, graph = await _ingest_client(connector)
    try:
        facts = [_fact("Ada", "wrote", "Note G", 10, 30)]
        first = await _submit(client, _batch(connector, 1, facts), "first")
        assert first.disposition is SourceIngestionDisposition.COMMITTED
        assert first.facts.created == 1
        assert first.facts.entries[0].outcome is FactOutcome.CREATED

        # The same content in a later batch changes nothing.
        second = await _submit(client, _batch(connector, 2, facts), "second")
        assert second.facts.unchanged == 1
        assert second.facts.created == 0
        assert second.facts.entries[0].outcome is FactOutcome.UNCHANGED
        assert second.facts.entries[0].unit_hash == first.facts.entries[0].unit_hash
        assert second.facts.entries[0].claim_id == first.facts.entries[0].claim_id

        # The same triple at another span updates the claim and keeps its node.
        moved = [_fact("Ada", "wrote", "Note G", 50, 70)]
        third = await _submit(client, _batch(connector, 3, moved), "third")
        assert third.facts.updated == 1
        assert third.facts.entries[0].outcome is FactOutcome.UPDATED
        assert third.facts.entries[0].claim_id == first.facts.entries[0].claim_id
        assert third.facts.entries[0].unit_hash != first.facts.entries[0].unit_hash
        claims = await client.nodes.list_by_label("CandidateClaim", 0)
        assert len(claims) == 1
        claim = await client.nodes.properties(claims[0][0])
        assert claim is not None
        assert claim["fact"]["confidence"] == pytest.approx(0.75)
    finally:
        await _close(client, graph)


@pytest.mark.spec("MEDULLA-INGEST-WORKER-R701.7")
@pytest.mark.asyncio
async def test_link_needs_shared_mentions() -> None:
    connector = _connector()
    client, graph = await _ingest_client(connector)
    try:
        facts = [
            # Ada and Note G share two atoms; Ada and Babbage share one.
            _fact("Ada", "wrote", "Note G", 10, 30),
            _fact("Ada", "annotated", "Note G", 40, 60),
            _fact("Ada", "met", "Babbage", 70, 90),
        ]
        receipt = await _submit(client, _batch(connector, 1, facts), "links")
        assert receipt.facts.created == 3
        assert receipt.facts.links == 1
        entities = {
            (await client.nodes.properties(node_id))["name"]: node_id
            for node_id, _ in await client.nodes.list_by_label("FactEntity", 0)
        }
        assert sorted(entities) == ["Ada", "Note G"]
        ada, note = entities["Ada"], entities["Note G"]
        assert await client.edges.has(ada, note) or await client.edges.has(note, ada)
    finally:
        await _close(client, graph)
