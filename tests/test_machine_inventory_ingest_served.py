"""Served ingest of the machine inventory records (MEDULLA-MACHINE-R006.4, R009).

The records of ``tests/fixtures/machine/records.json`` are the output of the
deterministic normalizer. They enter a tenant graph through the existing
``SourceIngest`` method under connector ``inventory-records``, stream ``machines``
and mode ``reconcile``. The tests read the stored graph back through served
``Sparql`` and validate it against the shipped machine shapes.
"""

from __future__ import annotations

import asyncio
import copy
import json
import os
import sys
import uuid
from pathlib import Path
from typing import Any

import pytest
from conftest import request_context
from pydantic import TypeAdapter

sys.path.insert(0, str(Path(__file__).resolve().parent / "typed_packs"))
sys.path.insert(0, str(Path(__file__).resolve().parent))
from _sparql_solutions import solution_values
from ontology_pack_support import connect, import_entries, ontology_entry

from medulla import ConnectorPackEntryContent
from medulla.client import EngineResponseError, MedullaClient
from medulla.generated.connector_pack import PackAnnotations, PackEntryKind
from medulla.generated.source_ingestion import SourceIngestionOutcome

ROOT = Path(__file__).resolve().parents[1]
RECORDS = json.loads(
    (ROOT / "tests/fixtures/machine/records.json").read_text(encoding="utf-8")
)
SHAPES = (ROOT / "crates/medulla-core/ontology/machine-v1.shapes.ttl").read_text(
    encoding="utf-8"
)
CORE = "https://knuckles-team.github.io/ontology/core#"
STREAM = "machines"
CONNECTOR = "inventory-records"
OWNERS = (
    "hasProcessor",
    "hasMemoryModule",
    "hasStorageDevice",
    "hasNetworkInterface",
    "hasNetworkBond",
    "hasAccelerator",
    "hasManagementController",
)


def _node_types(records: dict[str, Any]) -> dict[str, str]:
    return {entity["id"]: entity["node_type"] for entity in records["entities"]}


def _declaration(connector: str) -> ConnectorPackEntryContent:
    """One mapping per entity type, each onto the core class of the same name."""
    types = sorted(set(_node_types(RECORDS).values()))
    properties = sorted(
        {name for entity in RECORDS["entities"] for name in entity["properties"]}
    )
    mappings = {
        node_type: {
            "ontology_class": node_type,
            "fields": {name: name for name in properties},
        }
        for node_type in types
    }
    body = {"connector": connector, "schema_mappings": mappings, "sources": []}
    return ConnectorPackEntryContent(
        kind=PackEntryKind.RESOURCE,
        uri="connector://resources",
        output_schema=b'{"type":"object"}',
        name="connector-resources",
        media_type="application/json",
        body=json.dumps(body).encode(),
        annotations=PackAnnotations(),
    )


def _relations_ontology(connector: str) -> str:
    """The pack ontology: one single-range property per owner relation and link."""
    base = f"https://example.org/pack/{connector}#"
    classes = (
        "Machine",
        "CentralProcessor",
        "MemoryModule",
        "StorageDevice",
        "NetworkInterface",
        "NetworkBond",
        "Accelerator",
        "ManagementController",
        "MachineKey",
        "ProbeSnapshot",
    )
    lines = [
        "@prefix owl: <http://www.w3.org/2002/07/owl#> .",
        "@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .",
        f"@prefix core: <{CORE}> .",
        f"@prefix p: <{base}> .",
    ]
    for name in classes:
        lines.append(f"p:{name} a owl:Class ; rdfs:subClassOf core:{name} .")
    links = [
        (owner, "Machine", range_)
        for owner, range_ in zip(OWNERS, classes[1:9], strict=False)
    ]
    links += [
        ("bondMember", "NetworkBond", "NetworkInterface"),
        ("identifies", "MachineKey", "Machine"),
    ]
    for name, domain, range_ in links:
        lines.append(
            f"p:{name} a owl:ObjectProperty ; rdfs:subPropertyOf core:{name} ; "
            f"rdfs:domain p:{domain} ; rdfs:range p:{range_} ."
        )
    # `observes` has no domain in the core vocabulary; a pack names one property per
    # domain.
    lines.append(
        "p:observes a owl:ObjectProperty ; rdfs:subPropertyOf core:observes ; "
        "rdfs:domain p:ProbeSnapshot ; rdfs:range p:Machine ."
    )
    lines.append(f"@prefix v: <https://example.org/pack/{connector}/view#> .")
    lines.append(
        "v:observes a owl:ObjectProperty ; rdfs:subPropertyOf core:observes ; "
        "rdfs:domain p:Machine ; rdfs:range p:MachineKey ."
    )
    return "\n".join(lines) + "\n"


async def _tenant_client(connector: str) -> tuple[MedullaClient, str]:
    admin = await connect()
    graph = f"machine-{uuid.uuid4().hex[:8]}"
    try:
        await import_entries(
            admin,
            connector,
            [
                _declaration(connector),
                ontology_entry(
                    connector, "machine-links", _relations_ontology(connector)
                ),
            ],
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


def _provenance(connector: str) -> dict[str, Any]:
    return {
        "connector": connector,
        "adapter_kind": "mcp",
        "server": "inventory-build",
        "tool": "build_records",
        "tool_schema_sha256": "07" * 32,
        "source_uri": "inventory://hosts",
    }


def _batch(
    connector: str, records: dict[str, Any], page: int, previous: int | None
) -> dict[str, Any]:
    """One reconcile batch whose live ids are every entity id of the record set."""
    types = _node_types(records)
    checkpoint = {
        "stream": STREAM,
        "position": {"page": page},
        "watermark": "2026-11-27T10:00:00Z",
    }
    return {
        "connector": connector,
        "kind": "records",
        "mode": "reconcile",
        "strict_schema": False,
        "records": [
            {
                "stream": STREAM,
                "record_id": entity["id"],
                "mapping_reference": (
                    f"connector:{connector}#mappings/{entity['node_type']}"
                ),
                "payload": entity["properties"],
                "updated_at": "2026-11-27T10:00:00Z",
                "provenance": _provenance(connector),
            }
            for entity in records["entities"]
        ],
        "relationships": [
            {
                "source": {"stream": STREAM, "record_id": rel["source"]},
                "target": {"stream": STREAM, "record_id": rel["target"]},
                "relation_reference": (
                    f"connector:{connector}#relations/"
                    f"{types[rel['source']]}/{rel['relationship']}"
                ),
                "properties": {},
                "provenance": _provenance(connector),
            }
            for rel in records["relationships"]
        ],
        "withdrawals": [],
        "authoritative_live_ids": [
            {"stream": STREAM, "record_id": entity["id"]}
            for entity in records["entities"]
        ],
        "provider_checkpoint": checkpoint,
        "expected_previous_checkpoint": (
            None
            if previous is None
            else {
                "stream": STREAM,
                "position": {"page": previous},
                "watermark": "2026-11-27T10:00:00Z",
            }
        ),
    }


async def _submit(client: MedullaClient, batch: dict[str, Any], key: str) -> Any:
    """Submit one batch, retrying a stale scope version the pack projection can race."""
    for attempt in range(4):
        try:
            payload = await client._send(
                "SourceIngest",
                {"request": copy.deepcopy(batch)},
                idempotency_key=f"{key}:{uuid.uuid4().hex}",
            )
            break
        except EngineResponseError as error:
            if "STALE_VERSION" not in str(error) or attempt == 3:
                raise
            await asyncio.sleep(1 + attempt)
    outcome = TypeAdapter(SourceIngestionOutcome).validate_python(payload)
    assert outcome.outcome == "accepted", outcome
    return outcome.receipt


async def _rows(client: MedullaClient, query: str) -> list[dict[str, str | None]]:
    return solution_values(await client.rdf.sparql(query))


async def _stored(client: MedullaClient) -> dict[str, dict[str, list[str]]]:
    """The stored nodes by record id, with each predicate's values, read through SPARQL.

    A literal property (a list property included, one triple per element) comes from the
    node's RDF projection, and the class from its mapping reference; each edge target is
    named by its record id.
    """
    heads = await _rows(
        client, f"SELECT ?s ?rid WHERE {{ ?s <{CORE}sourceRecordId> ?rid }}"
    )
    nodes: dict[str, dict[str, list[str]]] = {head["rid"] or "": {} for head in heads}
    literals = await _rows(
        client,
        f"SELECT ?rid ?p ?o WHERE {{ ?s <{CORE}sourceRecordId> ?rid . "
        f"?s ?p ?o FILTER(isLiteral(?o)) }}",
    )
    for row in literals:
        predicate = (row["p"] or "").rsplit("#", 1)[-1]
        nodes[row["rid"] or ""].setdefault(predicate, []).append(row["o"] or "")
    # The class is the last segment of the mapping reference each record carries.
    for stored in nodes.values():
        reference = stored["sourceMappingReference"][0]
        stored["type"] = [reference.rsplit("/", 1)[-1]]
    edges = await _rows(
        client,
        f"SELECT ?s ?rid ?p ?ord WHERE {{ ?s <{CORE}sourceRecordId> ?rid . "
        f"?s ?p ?o . ?o <{CORE}sourceRecordId> ?ord }}",
    )
    for row in edges:
        predicate = (row["p"] or "").rsplit("#", 1)[-1]
        nodes[row["rid"] or ""].setdefault(predicate, []).append(row["ord"] or "")
    return nodes


def _turtle(nodes: dict[str, dict[str, list[str]]]) -> str:
    """The stored graph as Turtle with each node under its record id."""
    out = [
        f"@prefix : <{CORE}> .",
        "@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .",
    ]
    wanted = {
        *(name for entity in RECORDS["entities"] for name in entity["properties"]),
        "identifies",
        "bondMember",
        "observes",
        *OWNERS,
    }
    for rid, predicates in sorted(nodes.items()):
        out.append(f"<{rid}> a :{predicates['type'][0]} .")
        for predicate, values in sorted(predicates.items()):
            if predicate not in wanted:
                continue
            for value in sorted(values):
                if value.startswith("urn:"):
                    obj = f"<{value}>"
                elif predicate == "observedAt":
                    obj = f'"{value}"^^xsd:dateTime'
                elif value.lstrip("-").isdigit():
                    obj = value
                else:
                    obj = json.dumps(value)
                out.append(f"<{rid}> :{predicate} {obj} .")
    return "\n".join(out) + "\n"


async def _close(client: MedullaClient, graph: str) -> None:
    try:
        await client.tenants.delete(graph)
    finally:
        await client.close()


def _machine_id() -> str:
    return next(e["id"] for e in RECORDS["entities"] if e["node_type"] == "Machine")


@pytest.mark.spec("MEDULLA-MACHINE-R006.4")
@pytest.mark.asyncio
async def test_records_reconcile_into_the_tenant_graph() -> None:
    connector = f"{CONNECTOR}-{uuid.uuid4().hex[:6]}"
    client, graph = await _tenant_client(connector)
    try:
        first = receipt = await _submit(
            client, _batch(connector, RECORDS, 2, None), "first"
        )
        assert receipt.committed_graph_version > 0
        stored = await _stored(client)
        assert set(stored) == {entity["id"] for entity in RECORDS["entities"]}
        machine = stored[_machine_id()]
        assert machine["machineName"] == ["alpha"]
        assert machine["identityTier"] == ["serial"]
        assert machine["machineLifecycleState"] == ["active"]
        owned = [name for name in OWNERS for _ in machine.get(name, [])]
        assert len(owned) == 8
        assert "hasComponent" not in machine

        # A machine that leaves the record set is tombstoned at the next reconcile.
        trimmed = copy.deepcopy(RECORDS)
        gone = next(
            e["id"] for e in trimmed["entities"] if e["node_type"] == "Accelerator"
        )
        trimmed["entities"] = [e for e in trimmed["entities"] if e["id"] != gone]
        trimmed["relationships"] = [
            r
            for r in trimmed["relationships"]
            if gone not in (r["source"], r["target"])
        ]
        second = await _submit(client, _batch(connector, trimmed, 3, 2), "second")
        assert second.tombstoned_count == 1
        assert second.live_set_digest != first.live_set_digest
    finally:
        await _close(client, graph)


@pytest.mark.spec("MEDULLA-MACHINE-R006.4")
@pytest.mark.asyncio
async def test_the_graph_after_ingest_conforms_to_the_machine_shapes() -> None:
    connector = f"{CONNECTOR}-{uuid.uuid4().hex[:6]}"
    client, graph = await _tenant_client(connector)
    try:
        await _submit(client, _batch(connector, RECORDS, 2, None), "shapes")
        report = await client.rdf.validate_shacl(SHAPES, _turtle(await _stored(client)))
        assert report["conforms"] is True, report
    finally:
        await _close(client, graph)


CONNECTOR_VIEWS = (
    ("SshHost", "host:alpha"),
    ("ClusterMember", "host:alpha"),
    ("ManagedHost", "serial:ABC 123"),
)


@pytest.mark.spec("MEDULLA-MACHINE-R009")
@pytest.mark.asyncio
async def test_three_connector_records_resolve_to_one_machine_with_its_state() -> None:
    connector = f"{CONNECTOR}-{uuid.uuid4().hex[:6]}"
    client, graph = await _tenant_client(connector)
    try:
        anchors = {
            e["properties"]["machineKey"]: e["id"]
            for e in RECORDS["entities"]
            if e["node_type"] == "MachineKey"
        }
        # Three connector records each point `observes` at the key anchor they hold.
        combined = copy.deepcopy(RECORDS)
        for index, (kind, key) in enumerate(CONNECTOR_VIEWS):
            view = f"urn:connector-view:{kind}:{index}"
            combined["entities"].append(
                {
                    "id": view,
                    "node_type": "Machine",
                    "properties": {"machineName": kind},
                }
            )
            combined["relationships"].append(
                {"source": view, "relationship": "observes", "target": anchors[key]}
            )
        await _submit(client, _batch(connector, combined, 2, None), "machine")
        stored = await _stored(client)
        machines = {rid: p for rid, p in stored.items() if "identityTier" in p}
        assert len(machines) == 1, sorted(machines)
        machine_id, machine = next(iter(machines.items()))
        assert machine["machineName"] == ["alpha"]
        assert machine["machineLifecycleState"] == ["active"]
        keys = {
            rid for rid, p in stored.items() if machine_id in p.get("identifies", [])
        }
        observers = {
            rid
            for rid, p in stored.items()
            if any(target in keys for target in p.get("observes", []))
        }
        probes = {
            rid
            for rid, p in stored.items()
            if machine_id in p.get("observes", []) and p["type"] == ["ProbeSnapshot"]
        }
        assert len(observers) == 3, observers
        assert len(probes) == 1, probes
    finally:
        await _close(client, graph)
