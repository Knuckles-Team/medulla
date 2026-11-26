"""Served external schema import (MEDULLA-TYPED-PACKS-R097.2.7.4).

A pack pins one external schema through an ``external_schema`` entry: its terms of
every kind and the mappings the connector declares. The served import admits it,
the attach stores its coverage on the pack's schema source, and ``OntologyInspect``
returns the coverage exactly. Only a declared, kind-compatible mapping is adopted;
an unmapped term is reported and never becomes a platform term. A refusal changes
nothing.
"""

from __future__ import annotations

import asyncio
import hashlib
import json
import os
import uuid

import pytest
from conftest import TEST_TENANT, request_context
from ontology_pack_support import (
    attach_pack,
    connect,
    import_entries,
    violation_codes,
)

from medulla import ConnectorPackClient, ConnectorPackEntryContent
from medulla.client import MedullaClient
from medulla.generated import reasoning
from medulla.generated.connector_pack import PackAnnotations, PackEntryKind

CORE = "https://knuckles-team.github.io/ontology/core"
EXT = "http://example.org/ext#"
DIGEST = hashlib.sha256(b"external schema").hexdigest()

# Every kind of the closed set, each with a mapped and an unmapped term.
KINDS = [
    "class",
    "object_property",
    "datatype_property",
    "annotation_property",
    "named_individual",
]


def _body() -> dict:
    terms = []
    mappings = []
    for kind in KINDS:
        terms.append({"external_iri": f"{EXT}mapped_{kind}", "kind": kind})
        terms.append({"external_iri": f"{EXT}unmapped_{kind}", "kind": kind})
        mappings.append(
            {
                "schema_version": 1,
                "source_schema_digest": DIGEST,
                "source_revision": "ext-2026-01",
                "external_iri": f"{EXT}mapped_{kind}",
                "external_kind": kind,
                "platform_iri": f"{CORE}#platform_{kind}",
                "platform_kind": kind,
                "mapping_revision": "map-1",
            }
        )
    return {
        "schema_version": 1,
        "source_schema_digest": DIGEST,
        "source_revision": "ext-2026-01",
        "mapping_revision": "map-1",
        "terms": terms,
        "mappings": mappings,
    }


def _entry(connector: str, body: dict) -> ConnectorPackEntryContent:
    return ConnectorPackEntryContent(
        kind=PackEntryKind.EXTERNAL_SCHEMA,
        uri=f"external-schema://{connector}/ext.json",
        name="ext.json",
        media_type="application/json",
        body=json.dumps(body, sort_keys=True).encode(),
        annotations=PackAnnotations(),
    )


# proves R097.2.7.4: External schema import hot-path proof
@pytest.mark.spec("MEDULLA-TYPED-PACKS-R097.2.7.4")
def test_mapped_and_unmapped_terms_of_every_kind() -> None:
    async def exercise() -> None:
        tag = uuid.uuid4().hex[:8]
        connector = f"external-schema-{tag}"
        graph = f"external-schema-{tag}"
        admin = await connect()
        try:
            # A refusal changes nothing: no head exists afterwards.
            mismatched = _body()
            mismatched["terms"][0]["kind"] = "object_property"
            refused = await import_entries(
                admin, connector, [_entry(connector, mismatched)]
            )
            assert violation_codes(refused) == {"EXTERNAL_KIND_MISMATCH"}, refused
            outside = _body()
            outside["mappings"][0]["external_iri"] = f"{EXT}no_such_term"
            refused = await import_entries(
                admin, connector, [_entry(connector, outside)]
            )
            assert violation_codes(refused) == {"EXTERNAL_SOURCE_OUT_OF_SCOPE"}, refused
            status = await ConnectorPackClient(admin).status(
                tenant_id=TEST_TENANT, connector=connector
            )
            assert status.head is None

            imported = await import_entries(
                admin, connector, [_entry(connector, _body())]
            )
            assert imported["result"] == "imported", imported
            assert any(
                warning["code"] == "EXTERNAL_TERM_UNMAPPED"
                for warning in imported["receipt"]["warnings"]
            ), imported
            await admin.tenants.create(graph)
        finally:
            await admin.close()

        client = await MedullaClient.connect(
            socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
            verified_context=request_context(),
            graph_name=graph,
        )
        try:
            await attach_pack(client, connector)
            inspection = await reasoning.send_ontology_inspect(
                client, {"documents": [], "source_ids": [f"pack:{connector}"]}
            )
            assert [view.source_id for view in inspection.external_coverage] == [
                f"pack:{connector}"
            ]
            coverage = inspection.external_coverage[0].coverage.model_dump(mode="json")
            # The stored coverage is exactly the sorted terms with their results.
            assert [term["external_iri"] for term in coverage["terms"]] == sorted(
                term["external_iri"] for term in coverage["terms"]
            )
            assert len(coverage["terms"]) == 2 * len(KINDS)
            for term in coverage["terms"]:
                kind = term["kind"]
                if term["external_iri"] == f"{EXT}mapped_{kind}":
                    assert term["result"]["state"] == "mapped", term
                    assert term["result"]["platform_iri"] == f"{CORE}#platform_{kind}"
                    assert len(str(term["result"]["provenance_digest"])) == 64
                else:
                    assert term["external_iri"] == f"{EXT}unmapped_{kind}", term
                    assert term["result"]["state"] == "unmapped", term
            assert coverage["source_schema_digest"] == DIGEST
            assert len(str(coverage["coverage_digest"])) == 64

            # An unmapped external class is never a platform term.
            declared: set[str] = set()
            cursor = None
            while True:
                page = await reasoning.send_graph_schema_classes(
                    client, {"kind": "class", "cursor": cursor, "limit": 1000}
                )
                declared.update(term.iri for term in page.terms)
                cursor = page.next_cursor
                if cursor is None:
                    break
            assert f"{EXT}unmapped_class" not in declared
            assert f"{EXT}mapped_class" not in declared
        finally:
            await client.tenants.delete(graph)
            await client.close()

    asyncio.run(exercise())
