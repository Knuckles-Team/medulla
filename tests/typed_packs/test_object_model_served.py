"""Served object-model declarations of a connector pack (MEDULLA-TYPED-PACKS-R650.x).

A pack ontology may declare value types, link types and interfaces with the core
terms ``:ValueType``, ``:LinkType`` and ``:Interface``. The served import extracts
them once, cross-validates the shapes they compile to, and the attached pack
carries those shapes into the graph's guard policy. ``GraphSchemaClasses`` reports
each interface implementation with the required members its class lacks.
"""

from __future__ import annotations

import asyncio
import os
import uuid

import pytest
from conftest import TEST_TENANT, request_context
from ontology_pack_support import (
    attach_pack,
    connect,
    import_entries,
    ontology_entry,
    violation_codes,
)

from medulla import ConnectorPackClient
from medulla.client import EngineResponseError, MedullaClient
from medulla.generated import reasoning

CORE = "https://knuckles-team.github.io/ontology/core"
WORK_ITEM = f"{CORE}#WorkItem"
HEADER = (
    f"@prefix core: <{CORE}#> .\n"
    "@prefix owl: <http://www.w3.org/2002/07/owl#> .\n"
    "@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n"
    "@prefix sh: <http://www.w3.org/ns/shacl#> .\n"
    "@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .\n"
)
NAME_PROPERTY = (
    "ex:Name a core:ValueType ; sh:datatype xsd:string .\n"
    "ex:name a owl:DatatypeProperty ; rdfs:domain ex:Person ; rdfs:range ex:Name .\n"
)
DATA = (
    "@prefix ex: <{base}#> .\n"
    "@prefix rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .\n"
)


def _declarations(base: str, *extra: str) -> str:
    """A value type, a capped link and an interface a person class implements."""
    return (
        HEADER
        + f"@prefix ex: <{base}#> .\n"
        + f"ex:Asset a owl:Class ; rdfs:subClassOf <{WORK_ITEM}> .\n"
        + f"ex:Person a owl:Class ; rdfs:subClassOf <{WORK_ITEM}> .\n"
        + 'ex:Sku a core:ValueType ; sh:datatype xsd:string ; sh:pattern "^[A-Z]{3}-[0-9]+$" .\n'
        + "ex:sku a owl:DatatypeProperty ; rdfs:domain ex:Asset ; rdfs:range ex:Sku .\n"
        + "ex:owns a core:LinkType ; rdfs:domain ex:Person ; rdfs:range ex:Asset ; sh:maxCount 3 .\n"
        + "ex:Named a core:Interface ; core:hasInterfaceProperty ex:name , ex:owns .\n"
        + "ex:Person core:implementsInterface ex:Named .\n"
        + "".join(extra)
    )


async def _import_and_attach(connector: str, base: str, graph: str) -> MedullaClient:
    admin = await connect()
    try:
        imported = await import_entries(
            admin,
            connector,
            [
                ontology_entry(
                    connector, "vocabulary", _declarations(base, NAME_PROPERTY)
                )
            ],
        )
        assert imported["result"] == "imported", imported
        await admin.tenants.create(graph)
    finally:
        await admin.close()
    client = await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
        graph_name=graph,
    )
    await attach_pack(client, connector)
    return client


@pytest.mark.spec("MEDULLA-TYPED-PACKS-R650.3")
def test_import_compiles_shapes_and_graph_schema_classes_reports_interfaces() -> None:
    async def exercise() -> None:
        tag = uuid.uuid4().hex[:8]
        connector = f"object-model-{tag}"
        base = f"https://example.org/{connector}"
        graph = f"object-model-{tag}"
        admin = await connect()
        try:
            # A link type with no range is one named refusal and commits nothing.
            broken = await import_entries(
                admin,
                f"{connector}-broken",
                [
                    ontology_entry(
                        f"{connector}-broken",
                        "vocabulary",
                        HEADER
                        + f"@prefix ex: <{base}#> .\n"
                        + "ex:owns a core:LinkType ; rdfs:domain ex:Person .\n",
                    )
                ],
            )
            assert violation_codes(broken) == {"OBJECT_MODEL_INVALID"}, broken
            assert [v["uri"] for v in broken["violations"]] == [f"{base}#owns"]
        finally:
            await admin.close()

        client = await _import_and_attach(connector, base, graph)
        try:
            page = await reasoning.send_graph_schema_classes(
                client, {"kind": None, "cursor": None, "limit": 1}
            )
            reported = {
                (view.class_, view.interface): view.missing for view in page.interfaces
            }
            # The person declares every member the interface requires.
            assert reported == {(f"{base}#Person", f"{base}#Named"): []}
        finally:
            await client.tenants.delete(graph)
            await client.close()

    asyncio.run(exercise())


async def _add(client: MedullaClient, base: str, body: str) -> None:
    await client.rdf.add_triples(turtle=DATA.format(base=base) + body)


@pytest.mark.spec("MEDULLA-TYPED-PACKS-R650.4")
def test_value_link_and_interface_violations_refuse_with_unchanged_state() -> None:
    async def exercise() -> None:
        tag = uuid.uuid4().hex[:8]
        connector = f"object-model-{tag}"
        base = f"https://example.org/{connector}"
        graph = f"object-model-{tag}"
        admin = await connect()
        try:
            # The person lacks the interface's required name: nothing commits.
            missing = await import_entries(
                admin,
                connector,
                [ontology_entry(connector, "vocabulary", _declarations(base))],
            )
            assert violation_codes(missing) == {"INTERFACE_UNIMPLEMENTED"}, missing
            assert [v["uri"] for v in missing["violations"]] == [f"{base}#Person"]
            status = await ConnectorPackClient(admin).status(
                tenant_id=TEST_TENANT, connector=connector
            )
            assert status.head is None
        finally:
            await admin.close()

        client = await _import_and_attach(connector, base, graph)
        try:
            assets = "".join(
                f'<https://example.org/data/a{n}> rdf:type ex:Asset ; ex:sku "ABC-{n}" .\n'
                for n in range(1, 5)
            )
            await _add(client, base, assets)
            before = await client.rdf.get_rdf()

            with pytest.raises(EngineResponseError, match="VALUE_CONSTRAINT_VIOLATED"):
                await _add(
                    client,
                    base,
                    '<https://example.org/data/bad> rdf:type ex:Asset ; ex:sku "bad" .\n',
                )
            owned = ", ".join(f"<https://example.org/data/a{n}>" for n in range(1, 5))
            with pytest.raises(EngineResponseError, match="LINK_CARDINALITY_EXCEEDED"):
                await _add(
                    client,
                    base,
                    '<https://example.org/data/p1> rdf:type ex:Person ; ex:name "Ann" ; '
                    f"ex:owns {owned} .\n",
                )
            # Every refusal left the graph as it was.
            assert await client.rdf.get_rdf() == before

            await _add(
                client,
                base,
                '<https://example.org/data/p2> rdf:type ex:Person ; ex:name "Bo" ; '
                "ex:owns <https://example.org/data/a1> .\n",
            )
        finally:
            await client.tenants.delete(graph)
            await client.close()

    asyncio.run(exercise())
