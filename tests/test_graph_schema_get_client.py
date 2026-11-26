"""GraphSchemaGet client vectors (MEDULLA-TYPED-PACKS-R813).

Static client checks against a fake transport; no engine. The generated sender takes a
source key and nothing else, and decodes the source projection beside its stored
documents.
"""

from __future__ import annotations

import asyncio

import pytest
from _client_fixtures import PayloadTransport
from pydantic import ValidationError

from medulla.generated import reasoning
from medulla.generated.graph_schema import GraphSchemaSourceDocumentsView

pytestmark = pytest.mark.no_engine

DIGEST = "ab" * 32
DOCUMENTS = {
    "schema_version": 1,
    "graph": "tenant",
    "composed_digest": DIGEST,
    "source": {
        "source_id": "admin:alpha",
        "origin": {"origin": "admin", "name": "alpha"},
        "shapes_sha256": None,
        "ontology_sha256": DIGEST,
        "shapes_bytes": 0,
        "ontology_bytes": 61,
        "attached_at_ms": 7,
    },
    "ontology_ttl": (
        "<https://example.org/a#C> a <http://www.w3.org/2002/07/owl#Class> .\n"
    ),
}


# spec: MEDULLA-TYPED-PACKS-R813.3
@pytest.mark.spec("MEDULLA-TYPED-PACKS-R813")
def test_get_sends_the_source_key_and_decodes_the_documents() -> None:
    transport = PayloadTransport({"GraphSchemaGet": DOCUMENTS})
    params = {"source_id": "admin:alpha"}
    view = asyncio.run(reasoning.send_graph_schema_get(transport, params, "tenant"))
    assert transport.sent[0].method == "GraphSchemaGet"
    assert transport.sent[0].params == params
    assert isinstance(view, GraphSchemaSourceDocumentsView)
    assert view.source.source_id == "admin:alpha"
    assert view.shapes_ttl is None
    assert view.ontology_ttl is not None and view.ontology_ttl.endswith(".\n")


# spec: MEDULLA-TYPED-PACKS-R813.3
@pytest.mark.spec("MEDULLA-TYPED-PACKS-R813")
def test_get_refuses_a_missing_key_and_extra_fields_before_sending() -> None:
    transport = PayloadTransport({"GraphSchemaGet": DOCUMENTS})
    for params in ({}, {"source_id": "a", "all": True}):
        with pytest.raises(ValidationError):
            asyncio.run(reasoning.send_graph_schema_get(transport, params, "tenant"))
    assert not transport.sent


@pytest.mark.spec("MEDULLA-TYPED-PACKS-R813.5")
def test_get_decodes_the_redaction_marker_and_defaults_it_off() -> None:
    plain = GraphSchemaSourceDocumentsView.model_validate(DOCUMENTS)
    assert plain.source.provenance_redacted is False
    source = dict(plain.source.model_dump(), provenance_redacted=True)
    redacted_payload = {**DOCUMENTS, "source": source}
    redacted = GraphSchemaSourceDocumentsView.model_validate(redacted_payload)
    assert redacted.source.provenance_redacted is True
