"""Served `SourceIngest` facts batch grounded through a subclass.

MEDULLA-INGEST-WORKER-R701.10.
"""

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
from medulla.generated.source_ingestion import SourceIngestionDisposition

from .test_source_stream_versions_served import (
    STREAM,
    _close,
    _connector,
    _declaration,
    _submit,
)

ONTOLOGY = """
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix ex: <http://example.org/o#> .
ex:Dog rdfs:subClassOf ex:Animal .
ex:ownedBy rdfs:domain ex:Animal ; rdfs:range ex:Person .
ex:Rex a ex:Dog ; rdfs:label "Rex" .
ex:Ada a ex:Person ; rdfs:label "Ada" .
"""


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


def _batch(connector: str, facts: list[dict[str, Any]]) -> dict[str, Any]:
    return {
        "connector": connector,
        "kind": "facts",
        "mode": "delta",
        "strict_schema": True,
        "records": [],
        "relationships": [],
        "withdrawals": [],
        "extension": {"facts": facts},
        "provider_checkpoint": {"stream": STREAM, "position": {"facts": 1}},
        "expected_previous_checkpoint": None,
    }


@pytest.mark.spec("MEDULLA-INGEST-WORKER-R701.10")
@pytest.mark.asyncio
async def test_dog_fact_grounds_to_animal_domain() -> None:
    connector = _connector()
    admin = await connect()
    graph = f"subclass-grounding-{uuid.uuid4().hex[:8]}"
    try:
        await import_entries(
            admin,
            connector,
            [_declaration(connector), ontology_entry(connector, "animals", ONTOLOGY)],
        )
        await admin.tenants.create(graph)
    finally:
        await admin.close()
    client = await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
        graph_name=graph,
    )
    try:
        receipt = await _submit(
            client,
            _batch(connector, [_fact("Rex", "ownedBy", "Ada", 0, 20)]),
            "dog",
        )
        assert receipt.disposition is SourceIngestionDisposition.COMMITTED
        assert receipt.facts.created == 1
        claims = await client.nodes.list_by_label("CandidateClaim", 0)
        assert len(claims) == 1
        claim = await client.nodes.properties(claims[0][0])
        assert claim is not None
        # The Dog subject grounds to the declared Animal domain, not to its own class.
        assert claim["grounding"]["status"] == "grounded"
        assert claim["grounding"]["subject_class"].endswith("#Animal")
        assert claim["grounding"]["direction_repaired"] is False
    finally:
        await _close(client, graph)
