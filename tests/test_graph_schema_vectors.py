"""GraphSchema runtime-ontology client vectors (MEDULLA-TYPED-PACKS-R103.3).

Static client checks against a fake transport; no engine. The vectors pin the
wire shape the hand-written client sends for ``ImportOntology`` and
``WithdrawOntology``: the request carries no importer, and the content digest
is the SHA-256 of the ontology body.
"""

from __future__ import annotations

import asyncio
import hashlib

import pytest
from _client_fixtures import PayloadTransport

from medulla.client import RdfClient
from medulla.generated.graph_schema import (
    GraphSchemaOpImportOntology,
    GraphSchemaOpWithdrawOntology,
)

pytestmark = pytest.mark.no_engine

BODY = "<http://example.org/onto#Alpha> a <http://www.w3.org/2002/07/owl#Class> .\n"
COMMITTED = {
    "schema_version": 1,
    "graph": "tenant",
    "composed_digest": "07" * 32,
    "graph_version": 3,
    "changed": True,
}


@pytest.mark.spec("MEDULLA-TYPED-PACKS-R103.3")
def test_import_ontology_vector() -> None:
    transport = PayloadTransport({"GraphSchema": COMMITTED})
    client = RdfClient(transport)
    committed = asyncio.run(
        client.import_ontology(
            "weather",
            "http://example.org/onto",
            "1",
            BODY,
            expected_prior_revision=1,
            graph="tenant",
            idempotency_key="onto-1",
        )
    )
    withdrawn = asyncio.run(
        client.withdraw_ontology(
            "weather", "http://example.org/onto", 2, graph="tenant"
        )
    )
    assert committed == COMMITTED
    assert withdrawn == COMMITTED
    import_op = {
        "op": "import_ontology",
        "connector": "weather",
        "ontology_iri": "http://example.org/onto",
        "version": "1",
        "ontology_ttl": BODY,
        "shapes_ttl": None,
        "content_digest": hashlib.sha256(BODY.encode("utf-8")).hexdigest(),
        "expected_prior_revision": 1,
    }
    withdraw_op = {
        "op": "withdraw_ontology",
        "connector": "weather",
        "ontology_iri": "http://example.org/onto",
        "expected_revision": 2,
    }
    # Each vector is a valid closed wire model: no importer field exists.
    GraphSchemaOpImportOntology.model_validate(import_op)
    GraphSchemaOpWithdrawOntology.model_validate(withdraw_op)
    assert "importer" not in import_op
    assert transport.sent == [
        ("GraphSchema", {"op": import_op}, "tenant", "onto-1"),
        ("GraphSchema", {"op": withdraw_op}, "tenant", None),
    ]
