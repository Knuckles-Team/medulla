"""Served `SourceIngest` facts batches: MEDULLA-INGEST-WORKER-R701.5, R701.7."""

from __future__ import annotations

import os
import sys
import uuid
from pathlib import Path
from typing import Any

import pytest
from conftest import request_context

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "typed_packs"))
from ontology_pack_support import connect, import_entries, ontology_entry

from medulla.client import MedullaClient
from medulla.generated.source_ingestion import (
    FactOutcome,
    SourceIngestionDisposition,
)

from .test_source_stream_versions_served import (
    STREAM,
    _close,
    _connector,
    _declaration,
    _ingest_client,
    _submit,
)

ONTOLOGY = """
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix ex: <http://example.org/o#> .
ex:wrote rdfs:domain ex:Person ; rdfs:range ex:Work .
ex:Ada a ex:Person ; rdfs:label "Ada" .
ex:NoteG a ex:Work ; rdfs:label "Note G" .
"""


async def _grounded_client(connector: str) -> tuple[MedullaClient, str]:
    """A client on a fresh graph whose pack declares the person and work ontology."""
    admin = await connect()
    graph = f"fact-grounding-{uuid.uuid4().hex[:8]}"
    try:
        await import_entries(
            admin,
            connector,
            [_declaration(connector), ontology_entry(connector, "people", ONTOLOGY)],
        )
        await admin.tenants.create(graph)
    finally:
        await admin.close()
    client = await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
        graph_name=graph,
    )
    return client, graph


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


@pytest.mark.spec("MEDULLA-INGEST-WORKER-R701.6")
@pytest.mark.asyncio
async def test_direction_repair_and_ungrounded_claims() -> None:
    connector = _connector()
    client, graph = await _grounded_client(connector)
    try:
        facts = [
            # Swapped: the work is the subject, so direction repair grounds it.
            _fact("Note G", "wrote", "Ada", 10, 30),
            # No class for either name: an ungrounded claim.
            _fact("Somebody", "wrote", "Something", 40, 60),
            # The first triple again: one deduped unit.
            _fact("Note G", "wrote", "Ada", 70, 90),
        ]
        receipt = await _submit(client, _batch(connector, 1, facts), "grounding")
        assert receipt.facts.created == 2
        assert len(receipt.facts.entries) == 2
        claims = {}
        for node_id, _ in await client.nodes.list_by_label("CandidateClaim", 0):
            properties = await client.nodes.properties(node_id)
            assert properties is not None
            claims[properties["fact"]["subject"]] = properties["grounding"]
        assert claims["Note G"]["status"] == "grounded"
        assert claims["Note G"]["direction_repaired"] is True
        assert claims["Somebody"]["status"] == "ungrounded"
    finally:
        await _close(client, graph)
