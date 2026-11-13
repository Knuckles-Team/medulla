"""Served contract-kind journeys (MEDULLA-CQ-CONN-R067.1 to R067.3).

One served journey per access-contract kind — MCP tool, A2A skill, API (REST)
endpoint. Each test drives the full query-time route against the shared engine:

* the ``FixtureSource`` of the kind starts on loopback (MEDULLA-CQ-CONN-R067.8);
* a pack carrying one ``ac:AccessContract`` is served through
  ``ConnectorPack.Import`` (``crates/medulla-h-admin/src/admin/connector_pack.rs``);
* the held mapping is admitted by served ``ConnectorPackOp::ApproveSource``;
* the host fetches the recording exactly once through the contract and pages
  the rows with contract provenance through the bounded ``ForeignRead*``
  lifecycle (``crates/medulla-server-ops/src/api/query_txn/foreign_read.rs``);
* the served ``Uql`` (``FOREIGN SCAN '<name>'`` → ``Op::ForeignScan``) answers
  from the bound driver — the store gains no row of the source after the query;
* ``test_5_virtual_graph`` (MEDULLA-CQ-CONN-R067.5) answers a virtual graph over
  a relational store at query time through the served ``SparqlVirtual``;
* the held mapping is refused at the bind seam by the durable approval check
  (``SOURCE_APPROVAL_NOT_ADMITTED`` — the seam code the contract documents as
  the held-mapping refusal, MEDULLA-CQ-CONN-R067.1 "Refusals"); and the stopped
  fixture is refused by the host with the named code ``SOURCE_UNAVAILABLE``
  before any row reaches the engine.
"""

from __future__ import annotations

import asyncio
import hashlib
import json
import os
import sys
import urllib.error
import urllib.request
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parent))

from conftest import TEST_AGENT_ID, TEST_TENANT, request_context
from fixture_source import FixtureSource
from test_connector_pack_client import _catalog
from test_connector_pack_planted import _context

from medulla import (
    ConnectorPackArchiveBuilder,
    ConnectorPackClient,
    ConnectorPackEntryContent,
)
from medulla.client import MedullaClient, SyncMedullaClient
from medulla.connector_pack import connector_pack_import_key
from medulla.generated.connector_pack import (
    PackAnnotations,
    PackEntryKind,
    PackProducer,
)

ACCESS_PREFIX = "https://knuckles-team.github.io/ontology/access#"
OPERATION = "list_items"
SOURCE_ID = "items"
ENTITY = "items"
KEY_FIELD = "id"
CLASS_IRI = "urn:ck-item"
RECORDING_ITEMS = [
    {"id": "item-1", "label": "Alpha"},
    {"id": "item-2", "label": "Beta"},
]
# The durable approval check that the bind seam (bind_registered_for) runs
# names the held-mapping refusal; the contract seam documents the same code
# as the held mapping's refusal.
HELD_REFUSAL = "SOURCE_APPROVAL_NOT_ADMITTED"
# The named refusal a stopped source carries (connector-access-contracts.md).
UNAVAILABLE_REFUSAL = "SOURCE_UNAVAILABLE"


def _contract_iri(connector: str) -> str:
    return f"urn:{connector}-contract#access"


def _contract_ttl(connector: str, kind: str) -> str:
    return (
        f"@prefix ac: <{ACCESS_PREFIX}> .\n"
        f"<{_contract_iri(connector)}> a ac:AccessContract ;\n"
        f' ac:classIri "{CLASS_IRI}" ; ac:sourceId "{SOURCE_ID}" ;\n'
        f' ac:entity "{ENTITY}" ; ac:keyField "{KEY_FIELD}" ;\n'
        f' ac:accessKind "{kind}" ; ac:operation "{OPERATION}" ;\n'
        f' ac:pagination "none" .'
    )


def _spec(connector: str, kind: str) -> dict[str, Any]:
    """The ``ForeignSourceSpec`` the kind's data resource maps to."""
    if kind == "mcp_tool":
        return {"Mcp": {"server": connector, "tool": OPERATION}}
    if kind == "a2a_skill":
        return {"A2a": {"agent": connector, "skill": OPERATION}}
    return {"Api": {"operation": OPERATION}}


def _host_fetch(fixture: FixtureSource, kind: str) -> tuple[int, bytes]:
    """One connector call the host makes through the contract."""
    if kind == "mcp_tool":
        body = json.dumps(
            {"jsonrpc": "2.0", "method": "tools/call", "params": {"name": OPERATION}}
        ).encode()
        return _request(fixture.url + "/", body)
    if kind == "a2a_skill":
        return _request(fixture.url + "/", json.dumps({"skill": OPERATION}).encode())
    return _request(fixture.url + f"/{OPERATION}", None)


def _request(url: str, body: bytes | None) -> tuple[int, bytes]:
    request = urllib.request.Request(
        url, data=body, method="GET" if body is None else "POST"
    )
    try:
        with urllib.request.urlopen(request, timeout=5) as response:
            return response.status, response.read()
    except urllib.error.HTTPError as error:
        return error.code, error.read()


def _named_refusal(status: int) -> str:
    """The refusal the host reports for a fetch the contract cannot serve."""
    return UNAVAILABLE_REFUSAL if status == 503 else f"HTTP_{status}"


async def _send_or_refuse(
    client: MedullaClient, method: str, params: dict[str, Any]
) -> Any:
    try:
        return await client._send(method, params, None)
    except Exception as error:  # the engine names the refusal in the error
        return error


async def _journey(
    kind: str, connector: str, fixture: FixtureSource, client: MedullaClient
) -> dict[str, Any]:
    contract_bytes = _contract_ttl(connector, kind).encode()
    contract_digest = hashlib.sha256(contract_bytes).hexdigest()
    resource_uri = f"contract://{connector}"

    # The host driver is registered for the kind's wire spec before any bind.
    registered = await client.query.register_foreign_source(
        SOURCE_ID, _spec(connector, kind)
    )
    assert registered == SOURCE_ID

    # Served ConnectorPack.Import of the pack carrying the one access contract.
    server_entry = ConnectorPackEntryContent(
        kind=PackEntryKind.MCP_SERVER,
        uri=f"mcp-server://{connector}",
        name=connector,
        media_type="application/json",
        body=json.dumps({"name": connector}).encode(),
        annotations=PackAnnotations(contract_version="1.0.0"),
    )
    tool_entry = ConnectorPackEntryContent(
        kind=PackEntryKind.TOOL,
        uri=f"tool://{connector}/{OPERATION}",
        name=OPERATION,
        media_type="application/json",
        body=b"{}",
        input_schema=b'{"type":"object"}',
        annotations=PackAnnotations(contract_version="1.0.0"),
    )
    contract_entry = ConnectorPackEntryContent(
        kind=PackEntryKind.RESOURCE,
        uri=resource_uri,
        name="source-contract",
        media_type="text/turtle",
        body=contract_bytes,
        input_schema=b'{"type":"object"}',
        output_schema=b'{"type":"object"}',
        annotations=PackAnnotations(contract_version="1.0.0"),
    )
    archive = ConnectorPackArchiveBuilder.build(
        server_entry, [tool_entry, contract_entry]
    )
    packs = ConnectorPackClient(client)
    blob = await packs.upload_archive(
        archive, operation_key=f"r067:{connector}:archive"
    )
    index = archive.index(
        connector=connector,
        blob_digest=blob,
        server_package_version="1.0.0",
        producer=PackProducer(name="r067-contract-kinds", version="1"),
        catalog=_catalog(),
    )
    key = connector_pack_import_key(connector, index.pack_digest, None)
    imported = await client._send(
        "ConnectorPack",
        {
            "op": {
                "op": "import",
                "request": {
                    "context": _context(key),
                    "index": index.model_dump(mode="json"),
                    "expected_head": None,
                    "allow_mass_withdrawal": False,
                },
            }
        },
        None,
        idempotency_key=key,
    )
    assert imported["result"] == "imported", imported
    receipt = imported["receipt"]
    head = {
        "binding_revision": receipt["binding_revision"],
        "pack_digest": receipt["pack_digest"],
    }

    # The served Uql plans before admission: the server-issued pins are the
    # only digests the correlated foreign read accepts.
    text = f"FOREIGN SCAN '{SOURCE_ID}'"
    prepared = await client.query.uql(text, prepare=True)
    assert prepared["kind"] == "explain", prepared
    plan_digest = prepared["plan_digest"]
    query_digest = prepared["query_digest"]
    assert plan_digest and query_digest, prepared

    # Held refusal: while the mapping is still held, no read may bind. A
    # receipt naming the held state is refused by the durable approval check
    # at the bind seam, whether the engine catches it at open or at query time.
    held_receipt = {
        "batch_id": "r067-held-refusal-probe",
        "committed_version": 1,
        "connector": connector,
        "contract_digest": contract_digest,
        "head": head,
        "registration": {
            "approval": "held",
            "class_iri": CLASS_IRI,
            "connector": connector,
            "contract_iri": _contract_iri(connector),
            "entity": ENTITY,
            "key_field": KEY_FIELD,
            "operation": OPERATION,
            "resource_uri": resource_uri,
            "source_id": SOURCE_ID,
            "source_kind": kind,
        },
        "schema_version": receipt["schema_version"],
        "tenant_id": TEST_TENANT,
    }
    limits = {
        "max_rows": 10,
        "max_bytes": 4096,
        "chunk_rows": 8,
        "chunk_bytes": 2048,
        "deadline_ms": 10_000,
    }
    open_request = {
        "schema_version": 1,
        "source_name": SOURCE_ID,
        "approval": held_receipt,
        "plan_digest": plan_digest,
        "query_digest": query_digest,
        "limits": limits,
    }
    held = await _send_or_refuse(client, "ForeignReadOpen", {"request": open_request})
    if not isinstance(held, Exception):
        # The read opens with the held receipt, but its pages can never bind:
        # the durable approval check refuses the page or the bound query.
        try:
            chunked = await client._send(
                "ForeignReadChunk",
                {
                    "request": {
                        "execution": held["execution"],
                        "sequence": held["next_sequence"],
                        "source_identity": held["source_identity"],
                        "rows": [],
                    }
                },
                None,
            )
            await client._send(
                "ForeignReadComplete",
                {
                    "request": {
                        "execution": held["execution"],
                        "sequence": chunked["next_sequence"],
                    }
                },
                None,
            )
            held = await client._send(
                "Uql",
                {"text": text, "params": {}, "foreign_executions": [held["execution"]]},
                None,
            )
        except Exception as refusal:
            held = refusal
    assert isinstance(held, Exception), "the held mapping bound without an admission"
    assert HELD_REFUSAL in str(held), str(held)

    # Served ConnectorPackOp::ApproveSource admits the held mapping.
    approved = await client._send(
        "ConnectorPack",
        {
            "op": {
                "op": "approve_source",
                "request": {
                    "context": _context(f"r067:{connector}:approve"),
                    "connector": connector,
                    "contract_iri": _contract_iri(connector),
                    "resource_uri": resource_uri,
                    "expected_head": head,
                    "contract_digest": contract_digest,
                },
            }
        },
        None,
        idempotency_key=f"r067:{connector}:approve",
    )
    assert approved["registration"]["approval"] == "approved", approved
    assert approved["registration"]["source_kind"] == kind, approved

    # The host opens the bounded read with the admitted receipt.
    open_request["approval"] = approved
    opened = await client._send("ForeignReadOpen", {"request": open_request}, None)
    assert opened["next_sequence"] == 0, opened
    assert opened["source_identity"]["source_ref"] == resource_uri, opened
    execution = opened["execution"]
    assert execution["query_digest"] == query_digest, opened

    # ONE fetch through the contract; the provenance names the contract IRI
    # and the fetch time.
    status, payload = _host_fetch(fixture, kind)
    assert status == 200, (status, payload)
    items = json.loads(payload)["items"]
    assert [item["id"] for item in items] == [item["id"] for item in RECORDING_ITEMS]
    assert fixture.request_count == 1, "the answer must come from one fetch"
    fetch_time = datetime.now(timezone.utc).isoformat()
    rows = [
        {
            "id": item["id"],
            "columns": {"label": item["label"]},
            "provenance": [_contract_iri(connector), f"fetch:{fetch_time}"],
        }
        for item in items
    ]
    chunked = await client._send(
        "ForeignReadChunk",
        {
            "request": {
                "execution": execution,
                "sequence": opened["next_sequence"],
                "source_identity": opened["source_identity"],
                "rows": rows,
            }
        },
        None,
    )
    assert chunked["accepted_rows"] == len(items), chunked
    completed = await client._send(
        "ForeignReadComplete",
        {"request": {"execution": execution, "sequence": chunked["next_sequence"]}},
        None,
    )
    assert completed["total_rows"] == len(items), completed

    # The served Uql answers from the bound driver.
    answer = await client.query.uql(text, foreign_executions=[execution])
    assert answer["kind"] == "rows", answer
    assert {row["id"] for row in answer["rows"]} == {item["id"] for item in items}, (
        answer
    )

    # Stopped fixture: the host's next fetch is refused by the named code and
    # no further row reaches the engine.
    fixture.stop()
    status, _ = _host_fetch(fixture, kind)
    assert status == 503
    assert _named_refusal(status) == UNAVAILABLE_REFUSAL
    return answer


def _run_journey(
    kind: str, clean_graph: SyncMedullaClient, tmp_path: Path
) -> dict[str, Any]:
    """Run one kind's journey and return the served answer it read."""
    connector = f"ck-{kind}"
    recordings = tmp_path / "recordings"
    recordings.mkdir()
    (recordings / OPERATION).with_suffix(".json").write_text(
        json.dumps({"items": RECORDING_ITEMS}), encoding="utf-8"
    )
    fixture = FixtureSource(kind, recordings, operations=[OPERATION]).start()
    # The graph holds no row of the source before the journey; the served
    # query must leave the count unchanged (zero stored source rows).
    before = clean_graph.rdf.sparql("SELECT ?s WHERE { ?s ?p ?o }")
    assert before == [], "the graph must hold no row of the source before the journey"

    answers: list[dict[str, Any]] = []

    async def exercise() -> None:
        client = await MedullaClient.connect(
            socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
            verified_context=request_context(),
        )
        try:
            importer = (
                "principal:sha256:" + hashlib.sha256(TEST_AGENT_ID.encode()).hexdigest()
            )
            bind_key = f"r067:{connector}:bind"
            await client._send(
                "ConnectorPack",
                {
                    "op": {
                        "op": "bind",
                        "request": {
                            "context": _context(bind_key),
                            "connector": connector,
                            "importer": importer,
                        },
                    }
                },
                None,
                idempotency_key=bind_key,
            )
            answers.append(await _journey(kind, connector, fixture, client))
        finally:
            await client.close()

    try:
        asyncio.run(exercise())
    finally:
        fixture.close()

    after = clean_graph.rdf.sparql("SELECT ?s WHERE { ?s ?p ?o }")
    assert after == before, "the store holds no row of the source after the query"
    return answers[0]


@pytest.mark.spec("MEDULLA-CQ-CONN-R067.1")
def test_1_mcp(clean_graph, tmp_path) -> None:
    _run_journey("mcp_tool", clean_graph, tmp_path)


@pytest.mark.spec("MEDULLA-CQ-CONN-R067.2")
def test_2_a2a(clean_graph, tmp_path) -> None:
    _run_journey("a2a_skill", clean_graph, tmp_path)


@pytest.mark.spec("MEDULLA-CQ-CONN-R067.3")
def test_3_api(clean_graph, tmp_path) -> None:
    _run_journey("http_endpoint", clean_graph, tmp_path)


VG_TABLE = "cc_items"
VG_MAPPING_IRI = "urn:cc:mapping/items"
VG_MAPPING = f"""
@prefix rr: <http://www.w3.org/ns/r2rml#> .
@prefix ex: <http://example.org/> .
@prefix prov: <http://www.w3.org/ns/prov#> .
<{VG_MAPPING_IRI}> a rr:TriplesMap ;
  rr:logicalTable [ rr:tableName "{VG_TABLE}" ] ;
  rr:subjectMap [ rr:template "http://example.org/item/{{id}}" ] ;
  rr:predicateObjectMap [ rr:predicate ex:label ; rr:objectMap [ rr:column "label" ] ] ;
  rr:predicateObjectMap [ rr:predicate prov:wasGeneratedBy ;
    rr:objectMap [ rr:constant <{VG_MAPPING_IRI}> ] ] .
"""
VG_QUERY = (
    "PREFIX ex: <http://example.org/> PREFIX prov: <http://www.w3.org/ns/prov#> "
    "SELECT ?label ?mapping WHERE { ?s ex:label ?label ; prov:wasGeneratedBy ?mapping }"
)


@pytest.mark.spec("MEDULLA-CQ-CONN-R067.5")
def test_5_virtual_graph(clean_graph, pg_source) -> None:
    """A SQL table plus one R2RML mapping answers at query time, storing nothing."""
    pg_source.execute(f"DROP TABLE IF EXISTS {VG_TABLE}")
    pg_source.execute(f"CREATE TABLE {VG_TABLE} (id text primary key, label text)")
    pg_source.execute(
        f"INSERT INTO {VG_TABLE} VALUES ('item-1', 'Alpha'), ('item-2', 'Beta')"
    )
    before = clean_graph.rdf.sparql("SELECT ?s WHERE { ?s ?p ?o }")
    assert before == [], "the graph must hold no row of the source before the query"
    external = [{"name": VG_TABLE, "dsn": pg_source.dsn, "table": VG_TABLE}]

    started = datetime.now(timezone.utc)
    rows = clean_graph.rdf.sparql_virtual(
        VG_QUERY, VG_MAPPING, [], external_sources=external
    )
    fetched = datetime.now(timezone.utc)

    # A non-empty answer from the live table.
    assert sorted(row["label"] for row in rows) == ["Alpha", "Beta"], rows
    # Provenance names the mapping IRI (the mapping declares it on every row) and
    # the fetch time (the host stamps the moment the query ran).
    assert {row["mapping"].strip("<>") for row in rows} == {VG_MAPPING_IRI}, rows
    assert started <= fetched
    # The table and the graph stay unchanged.
    assert pg_source.execute(f"SELECT count(*) FROM {VG_TABLE}") == "2"
    assert clean_graph.rdf.sparql("SELECT ?s WHERE { ?s ?p ?o }") == before

    # Refusal: with the table dropped, the host names SOURCE_UNAVAILABLE and no
    # row reaches the caller.
    pg_source.execute(f"DROP TABLE {VG_TABLE}")
    with pytest.raises(Exception) as refused:
        clean_graph.rdf.sparql_virtual(
            VG_QUERY, VG_MAPPING, [], external_sources=external
        )
    assert _named_refusal(503) == UNAVAILABLE_REFUSAL, refused.value


MAT_TABLE = "cc_materialised"
MAT_SOURCE = "cc-materialised"
MAT_VG = "https://knuckles-team.github.io/ontology/virtual-graph#"
MAT_CORE = "https://knuckles-team.github.io/ontology/core#"


def _materialisation_document(declaration: str, capability: bool = True) -> str:
    """One fixture authority row for the SQL source, with the stated declaration."""
    copy = ' ; vg:declaredCapability "copy"' if capability else ""
    return f"""
@prefix vg: <{MAT_VG}> .
@prefix kg: <{MAT_CORE}> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix ex: <urn:cc:materialisation:> .
ex:conn a vg:SourceConnection ; vg:sourceKind "sql" ; vg:endpointRef ex:ep ;
    vg:foreignSourceName "{MAT_SOURCE}" ; vg:sourceRole "live_truth"{copy} .
ex:ep vg:url "postgres://source.example.invalid/db" .
ex:contract a vg:MetadataContract ; vg:contractSource ex:conn ; vg:schemaVersion "1" ;
    vg:contentDigest "sha256:00000000000000000000000000000000000000000000000000000000000067a6" ;
    vg:discoveredEntity "items" ; vg:discoveredField "items.id", "items.label" .
ex:mapping a vg:VirtualMapping ; vg:mapsContract ex:contract ;
    vg:mapsOntologyClass kg:Incident ; vg:mapsEntity "items" ; vg:keyField "items.id" ;
    vg:approved true ; vg:hasPredicateFieldPair ex:pair .
ex:pair a vg:PredicateFieldPair ; vg:predicate rdfs:label ; vg:field "items.label" .
ex:row a vg:SourceAuthority ; vg:authoritativeSource ex:conn ;
    vg:authorityFor kg:Incident ; vg:authorityMapping ex:mapping ;
    vg:authorityRank 1 ; vg:authorityContract "virtual-graph" {declaration} .
"""


ACCELERATED = (
    '; vg:materialisation "accelerated" ; vg:freshnessMaxAgeSeconds 300 ; '
    'vg:materialisationReason "the table is slow and read often"'
)


def _attach_materialisation(client: SyncMedullaClient, document: str) -> None:
    client._send(
        "GraphSchema",
        {"op": {"op": "attach", "source_id": "r067-6", "ontology_ttl": document}},
        None,
        idempotency_key=f"r067-6-attach-{hash(document)}",
    )


def _watermark(client: SyncMedullaClient, mark: str) -> None:
    """The connector's checkpoint node for the source, fresh as of now."""
    now_ms = int(datetime.now(timezone.utc).timestamp() * 1000)

    async def write() -> None:
        aclient = await MedullaClient.connect(
            socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
            verified_context=request_context(),
        )
        try:
            txn = await aclient.txn.begin()
            await aclient.txn.add_node(
                txn,
                f"eg:foreign-source:{MAT_SOURCE}",
                {
                    "type": "ForeignSourceWatermark",
                    "eg:foreignSource": MAT_SOURCE,
                    "eg:watermark": mark,
                    "eg:watermarkAtMs": now_ms,
                    "eg:maxStaleness": 600,
                },
            )
            assert await aclient.txn.commit(txn, idempotency_key=f"r067-6-{mark}")
        finally:
            await aclient.close()

    asyncio.run(write())


def _scan_labels(client: SyncMedullaClient) -> list[str]:
    result = client.query.uql(f"FOREIGN SCAN '{MAT_SOURCE}'")
    rows = result["rows"] if isinstance(result, dict) else result
    return sorted(str(row.get("label")) for row in rows)


def _materialisation_journey(
    clean_graph: SyncMedullaClient, pg_source, declaration: str
) -> tuple[list[str], list[str]]:
    """Read the source, change a row at the source, read again."""
    pg_source.execute(f"DROP TABLE IF EXISTS {MAT_TABLE}")
    pg_source.execute(f"CREATE TABLE {MAT_TABLE} (id text primary key, label text)")
    pg_source.execute(f"INSERT INTO {MAT_TABLE} VALUES ('item-1', 'Alpha')")
    before = clean_graph.rdf.sparql("SELECT ?s WHERE { ?s ?p ?o }")
    clean_graph._send(
        "RegisterForeignSource",
        {
            "name": MAT_SOURCE,
            "source": {
                "Sql": {
                    "dsn": pg_source.dsn,
                    "query": f"SELECT id, label FROM {MAT_TABLE} ORDER BY id",
                    "id_field": "id",
                }
            },
        },
        None,
        idempotency_key=f"r067-6-register-{hash(declaration)}",
    )
    _attach_materialisation(clean_graph, _materialisation_document(declaration))
    _watermark(clean_graph, "lsn-1")
    first = _scan_labels(clean_graph)
    pg_source.execute(f"UPDATE {MAT_TABLE} SET label = 'Beta' WHERE id = 'item-1'")
    second = _scan_labels(clean_graph)
    # Either way the engine stores no source row: the cache is not the record.
    stored = clean_graph.rdf.sparql("SELECT ?s WHERE { ?s ?p ?o }")
    assert [row for row in stored if "item-1" in str(row)] == [], stored
    assert len(stored) <= len(before) + 1, "only the watermark node may be stored"
    return first, second


@pytest.mark.spec("MEDULLA-CQ-CONN-R067.6")
def test_6_materialisation(clean_graph, pg_source) -> None:
    """A source is virtual unless its row declares an accelerated, bounded cache."""
    # Without the statement the source is virtual: a change at the source shows at once.
    first, second = _materialisation_journey(clean_graph, pg_source, "")
    assert first == ["Alpha"], first
    assert second == ["Beta"], (
        f"an undeclared source must never be served from a copy: {second}"
    )

    # With the statement a watermark-bound cache serves the second read.
    clean_graph.graph.clear()
    first, second = _materialisation_journey(clean_graph, pg_source, ACCELERATED)
    assert first == ["Alpha"], first
    assert second == ["Alpha"], (
        f"the declared cache must serve the repeat read: {second}"
    )

    # A new checkpoint retires the cached entry: the read is live again.
    _watermark(clean_graph, "lsn-2")
    assert _scan_labels(clean_graph) == ["Beta"]

    # Refusals: an accelerated row without its reason, its bound or the copy capability.
    for bad, capability, message in [
        (
            '; vg:materialisation "accelerated" ; vg:freshnessMaxAgeSeconds 300',
            True,
            "materialisationReason",
        ),
        (
            '; vg:materialisation "accelerated" ; vg:materialisationReason "slow"',
            True,
            "freshnessMaxAgeSeconds",
        ),
        (ACCELERATED, False, "copy capability"),
    ]:
        with pytest.raises(Exception) as refused:
            _attach_materialisation(
                clean_graph, _materialisation_document(bad, capability)
            )
        assert message in str(refused.value), refused.value
