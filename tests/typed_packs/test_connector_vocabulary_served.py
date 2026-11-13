"""Served vocabulary coverage for the modules of R071 to R081 (MEDULLA-CQ-CONN-R089).

One table-driven case per term, keyed by the term name. Each case imports a
fixture pack through the served ``ConnectorPack.Import``: its ontology class
extends the term's class (or, for a property, the property's domain class), so
the import itself proves the term resolves in the core catalog (a pack class
with no path to the catalog is refused ``ONTOLOGY_ORPHAN``). The case then
ingests one recorded record of that class through the served ``SourceIngest``
and reads it back: the served ``Uql`` finds the node by its type and the node's
properties carry the source, the record id, the as-of time and, for a
property term, the value under the term IRI.

Source nodes are property-graph nodes, not RDF resources, so the readback is
served UQL plus the node properties rather than a SPARQL projection. Shape
cases also send a violating record through the served ``ShaclValidate`` and
expect the shape to name the property.
"""

from __future__ import annotations

import json
import os
import uuid
from dataclasses import dataclass, field
from typing import Any

import pytest
from conftest import request_context
from ontology_pack_support import (
    connect,
    import_entries,
    ontology_entry,
    violation_codes,
)

from medulla import ConnectorPackEntryContent
from medulla.client import MedullaClient
from medulla.generated.connector_pack import PackAnnotations, PackEntryKind

KG = "https://knuckles-team.github.io/ontology/core#"
RDF_TYPE = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type"
STREAM = "records"
UPDATED_AT = "2026-09-20T00:00:00Z"


@dataclass(frozen=True)
class Term:
    """One vocabulary term of R071 to R081 (or of the commercial module)."""

    module: str
    name: str
    kind: str  # "class" | "object" | "data"
    domain: str | None = None  # the class a property term hangs on
    sample: str = "sample"
    connector: str = "vocab"
    # ShaclValidate cases: (triples as (suffix-or-IRI, predicate, object), message key)
    shape: tuple[tuple[tuple[str, str, str], ...], str] | None = field(default=None)

    @property
    def record_class(self) -> str:
        return self.name if self.kind == "class" else str(self.domain)


def _cls(module: str, name: str, **kw: Any) -> Term:
    return Term(module, name, "class", **kw)


def _obj(module: str, name: str, domain: str, **kw: Any) -> Term:
    return Term(module, name, "object", domain, **kw)


def _data(
    module: str, name: str, domain: str, sample: str = "sample", **kw: Any
) -> Term:
    return Term(module, name, "data", domain, sample, **kw)


def _typed(name: str) -> tuple[str, str, str]:
    return ("rec", RDF_TYPE, KG + name)


TERMS: list[Term] = [
    # documentation (R071)
    _cls("documentation", "WebSnapshot", shape=((_typed("WebSnapshot"),), "sourceRef")),
    _cls(
        "documentation",
        "ArchiveResult",
        shape=((_typed("ArchiveResult"),), "extractorName"),
    ),
    _obj("documentation", "hasArchiveResult", "WebSnapshot"),
    _data("documentation", "extractorName", "ArchiveResult"),
    _data(
        "documentation",
        "resultStatus",
        "ArchiveResult",
        "failed",
        shape=(
            (
                _typed("ArchiveResult"),
                ("rec", KG + "extractorName", "wget"),
                ("rec", KG + "resultStatus", "bogus"),
            ),
            "resultStatus",
        ),
    ),
    # process_intelligence (R072)
    _cls("process_intelligence", "EpcFunction"),
    _cls("process_intelligence", "EpcRuleOperator"),
    _obj("process_intelligence", "flowsTo", "EpcFunction"),
    _cls("process_intelligence", "ProcessInstance"),
    _obj("process_intelligence", "instanceOfProcessModel", "ProcessInstance"),
    _obj("process_intelligence", "stoppedAtActivity", "ProcessInstance"),
    _obj("process_intelligence", "raisedIncident", "ProcessInstance"),
    # government (R073)
    _cls("government", "RiskScenario"),
    _obj("government", "mitigatesRisk", "SecurityControl"),
    _obj("government", "inFramework", "SecurityControl"),
    _obj("government", "riskAffectsService", "RiskScenario"),
    _cls("government", "PrivacyAssessment"),
    _cls("government", "ProcessingActivity"),
    _obj("government", "assessmentCovers", "PrivacyAssessment"),
    _data("government", "assessmentDueDate", "PrivacyAssessment", "2026-12-31"),
    _obj("government", "assessedVendor", "PrivacyAssessment"),
    _data("government", "assessmentStatus", "PrivacyAssessment", "draft"),
    # core-foundation (R074)
    _cls("core-foundation", "GlossaryTerm"),
    _obj("core-foundation", "termAssignedTo", "GlossaryTerm"),
    _obj("core-foundation", "counterparty", "Agreement"),
    _data("core-foundation", "agreementExpiresAt", "Agreement", "2027-01-31"),
    _obj("core-foundation", "hasRenewalAction", "Agreement"),
    # finance (R075)
    _data(
        "finance",
        "orderQty",
        "VersionedOrder",
        "5",
        shape=(
            (
                _typed("VersionedOrder"),
                ("rec", KG + "orderQty", "5"),
                ("rec", KG + "filledQty", "9"),
            ),
            "filledQty",
        ),
    ),
    _cls("finance", "Budget", shape=((_typed("Budget"),), "budgetLimit")),
    _data("finance", "budgetLimit", "Budget", "250.00"),
    _cls("finance", "LedgerTransaction"),
    _obj("finance", "chargedToBudget", "LedgerTransaction"),
    _data("finance", "transactionAmount", "LedgerTransaction", "12.50"),
    # personal (R076)
    _obj("personal", "birthPlace", "Person"),
    _data("personal", "birthYear", "Person", "1980"),
    _data("personal", "documentType", "Document", "invoice"),
    _data("personal", "dueDate", "Document", "2026-12-01"),
    _data("personal", "totalAmount", "Document", "99.90"),
    _cls("personal", "FileShare"),
    _obj("personal", "sharesFile", "FileShare"),
    _data("personal", "isPublicLink", "FileShare", "true"),
    _data("personal", "shareExpiresAt", "FileShare", "2027-02-01"),
    # virtual_graph (R077)
    _cls("virtual_graph", "NamedGraph"),
    _data("virtual_graph", "tripleCount", "NamedGraph", "42"),
    _data("virtual_graph", "lastModifiedAt", "NamedGraph", UPDATED_AT),
    # identity (R078)
    _cls("identity", "OidcClient"),
    _obj("identity", "roleGrantsAccessTo", "IdentityRole"),
    _data(
        "identity",
        "accountStatus",
        "IdentityUser",
        "ACTIVE",
        shape=(
            (
                _typed("IdentityUser"),
                ("rec", KG + "accountStatus", "BOGUS"),
            ),
            "accountStatus",
        ),
    ),
    # orchestration (R079)
    _data("orchestration", "promptVersion", "Prompt", "3"),
    _data("orchestration", "tokenCost", "RunTrace", "0.0042"),
    _obj("orchestration", "tracePrompt", "RunTrace"),
    _data("orchestration", "evaluationScore", "RunTrace", "0.91"),
    # enterprise (R080)
    _data("enterprise", "factSheetType", "EAFactSheet", "Application"),
    _data(
        "enterprise",
        "lifecyclePhase",
        "EAFactSheet",
        "active",
        shape=(
            (
                _typed("EAFactSheet"),
                ("rec", KG + "lifecyclePhase", "bogus"),
            ),
            "lifecyclePhase",
        ),
    ),
    _data("enterprise", "activeUserCount", "EAFactSheet", "120"),
    # nutrition (R081)
    _cls("nutrition", "Recipe"),
    _obj("nutrition", "usesIngredient", "Recipe"),
    _cls("nutrition", "ShoppingListItem"),
    _cls("nutrition", "MealPlanEntry"),
    _cls("nutrition", "WorkoutRoutine"),
    _cls("nutrition", "Exercise"),
    _obj("nutrition", "includesExercise", "WorkoutRoutine"),
    _cls("nutrition", "WorkoutSession"),
    # commercial (MEDULLA-SUPPLY-VOCAB-R003.7 to R003.12), reached by the three
    # commercial connectors through the same registered module
    _cls("commercial", "Supplier", connector="erpnext"),
    _cls("commercial", "PurchaseOrder", connector="erpnext"),
    _cls("commercial", "PurchaseInvoice", connector="erpnext"),
    _obj("commercial", "invoiceSupplier", "PurchaseInvoice", connector="erpnext"),
    _cls("commercial", "Opportunity", connector="salesforce"),
    _obj("commercial", "opportunityCustomer", "Opportunity", connector="salesforce"),
    _obj("commercial", "contactOf", "Person", connector="twenty"),
    _data(
        "commercial",
        "customerLastActivityAt",
        "Customer",
        UPDATED_AT,
        connector="twenty",
    ),
]


def _resources(
    connector: str, pack_class: str, term: Term
) -> ConnectorPackEntryContent:
    fields = {"id": "identifier"}
    if term.kind != "class":
        fields["value"] = KG + term.name
    body = {
        "connector": connector,
        "schema_mappings": {"record": {"fields": fields, "ontology_class": pack_class}},
        "sources": [],
    }
    return ConnectorPackEntryContent(
        kind=PackEntryKind.RESOURCE,
        uri="connector://resources",
        name="connector-resources",
        media_type="application/json",
        body=json.dumps(body).encode(),
        annotations=PackAnnotations(),
    )


def _ontology(
    connector: str, pack_class: str, parent: str
) -> ConnectorPackEntryContent:
    turtle = (
        "@prefix owl: <http://www.w3.org/2002/07/owl#> .\n"
        "@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n"
        f"<{pack_class}> a owl:Class ; rdfs:subClassOf <{KG}{parent}> .\n"
    )
    return ontology_entry(connector, "vocabulary", turtle)


def _batch(connector: str, term: Term, value: str | None) -> dict[str, Any]:
    payload: dict[str, Any] = {"id": "rec-1"}
    if value is not None:
        payload["value"] = value
    return {
        "connector": connector,
        "kind": "records",
        "mode": "delta",
        "strict_schema": True,
        "records": [
            {
                "stream": STREAM,
                "record_id": "rec-1",
                "mapping_reference": f"connector:{connector}#mappings/record",
                "payload": payload,
                "updated_at": UPDATED_AT,
                "provenance": {
                    "connector": connector,
                    "adapter_kind": "mcp",
                    "server": f"{connector}-server",
                    "tool": "list_records",
                    "tool_schema_sha256": "07" * 32,
                    "source_uri": f"{connector}://records/rec-1",
                },
            }
        ],
        "relationships": [],
        "withdrawals": [],
        "provider_checkpoint": {
            "stream": STREAM,
            "position": {"page": 1},
            "watermark": UPDATED_AT,
        },
        "expected_previous_checkpoint": None,
    }


async def _ingest_graph(connector: str, entries: list[ConnectorPackEntryContent]):
    admin = await connect()
    graph = f"vocab-{uuid.uuid4().hex[:10]}"
    try:
        imported = await import_entries(admin, connector, entries)
        await admin.tenants.create(graph)
    finally:
        await admin.close()
    client = await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
        graph_name=graph,
    )
    return client, graph, imported


async def _read_back(client: MedullaClient, pack_class: str) -> dict[str, Any]:
    """The properties of the one node of ``pack_class`` (found by served Uql)."""
    answer = await client.query.uql(f"MATCH () WHERE type = '{pack_class}'")
    assert answer["kind"] == "rows", answer
    ids = [row["id"] for row in answer["rows"]]
    assert len(ids) == 1, answer
    props = await client.nodes.properties(ids[0])
    assert props is not None, ids
    return props


def _sample_value(term: Term, connector: str) -> str | None:
    if term.kind == "class":
        return None
    if term.kind == "object":
        return f"https://example.org/{connector}/target"
    return term.sample


@pytest.mark.spec("MEDULLA-CQ-CONN-R089")
@pytest.mark.asyncio
@pytest.mark.parametrize("term", TERMS, ids=[t.name for t in TERMS])
async def test_pack_record_of_each_new_term_round_trips(term: Term) -> None:
    connector = f"{term.connector}-{uuid.uuid4().hex[:8]}"
    pack_class = f"https://example.org/{connector}/{term.record_class}Record"
    entries = [
        _resources(connector, pack_class, term),
        _ontology(connector, pack_class, term.record_class),
    ]
    client, graph, imported = await _ingest_graph(connector, entries)
    try:
        # The pack class extends the term's class: the import is not an orphan.
        assert imported["result"] == "imported", imported

        value = _sample_value(term, connector)
        receipt = await client._send(
            "SourceIngest",
            {"request": _batch(connector, term, value)},
            idempotency_key=f"vocab:{connector}:{uuid.uuid4().hex}",
        )
        assert receipt["disposition"] == "committed", receipt

        props = await _read_back(client, pack_class)
        # Source, record id and as-of ride on the row.
        assert props["source_record_id"] == "rec-1", props
        assert props["source_stream"] == STREAM, props
        assert props["source_updated_at"] == UPDATED_AT, props
        assert connector in json.dumps(props["source_provenance"]), props
        if term.kind != "class":
            assert str(props[KG + term.name]) == str(value), props

        if term.shape is not None:
            triples, message_key = term.shape
            data = [
                {
                    "subject": f"https://example.org/{connector}/{subject}",
                    "predicate": predicate,
                    "object": obj,
                }
                for subject, predicate, obj in triples
            ]
            report = await client.rdf.validate_committed(data_triples=data)
            assert report["conforms"] is False, report
            assert message_key in json.dumps(report), report
    finally:
        await client.tenants.delete(graph)
        await client.close()


@pytest.mark.spec("MEDULLA-CQ-CONN-R089")
@pytest.mark.asyncio
async def test_pack_class_without_the_term_is_an_orphan() -> None:
    connector = f"orphan-{uuid.uuid4().hex[:8]}"
    turtle = (
        "@prefix owl: <http://www.w3.org/2002/07/owl#> .\n"
        f"<https://example.org/{connector}/Lonely> a owl:Class .\n"
    )
    admin = await connect()
    try:
        result = await import_entries(
            admin, connector, [ontology_entry(connector, "vocabulary", turtle)]
        )
    finally:
        await admin.close()
    assert "ONTOLOGY_ORPHAN" in violation_codes(result)


@pytest.mark.spec("MEDULLA-CQ-CONN-R089")
@pytest.mark.asyncio
async def test_record_lacking_the_property_stores_no_value() -> None:
    term = next(t for t in TERMS if t.name == "budgetLimit")
    connector = f"{term.connector}-{uuid.uuid4().hex[:8]}"
    pack_class = f"https://example.org/{connector}/BudgetRecord"
    client, graph, imported = await _ingest_graph(
        connector,
        [
            _resources(connector, pack_class, term),
            _ontology(connector, pack_class, term.record_class),
        ],
    )
    try:
        assert imported["result"] == "imported", imported
        # A schema mapping that names the property only maps it when present.
        batch = _batch(connector, term, None)
        await client._send(
            "SourceIngest",
            {"request": batch},
            idempotency_key=f"vocab:{connector}:{uuid.uuid4().hex}",
        )
        props = await _read_back(client, pack_class)
        assert KG + term.name not in props, props
    finally:
        await client.tenants.delete(graph)
        await client.close()
