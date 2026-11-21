"""Served object-model declarations of a connector pack (MEDULLA-TYPED-PACKS-R650.x).

A pack ontology may declare value types, link types and interfaces with the core
terms ``:ValueType``, ``:LinkType`` and ``:Interface``. The served import extracts
them once, cross-validates the shapes they compile to, and the attached pack
carries those shapes into the graph's guard policy. ``GraphSchemaClasses`` reports
each interface implementation with the required members its class lacks.
"""

from __future__ import annotations

import asyncio
import hashlib
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
        + "ex:Sku a core:ValueType ; sh:datatype xsd:string ; sh:pattern "
        '"^[A-Z]{3}-[0-9]+$" .\n'
        + "ex:sku a owl:DatatypeProperty ; rdfs:domain ex:Asset ; rdfs:range ex:Sku .\n"
        + "ex:owns a core:LinkType ; rdfs:domain ex:Person ; rdfs:range ex:Asset ; "
        "sh:maxCount 3 .\n"
        + "ex:Named a core:Interface ; core:hasInterfaceProperty ex:name , ex:owns .\n"
        + "ex:Person core:implementsInterface ex:Named .\n"
        + "".join(extra)
    )


async def _import_and_attach(
    connector: str, base: str, graph: str, receipts: list[dict] | None = None
) -> MedullaClient:
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
        if receipts is not None:
            receipts.append(imported["receipt"])
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
                f"<https://example.org/data/a{n}> rdf:type ex:Asset ; ex:sku "
                f'"ABC-{n}" .\n'
                for n in range(1, 5)
            )
            await _add(client, base, assets)
            before = await client.rdf.get_rdf()

            with pytest.raises(EngineResponseError, match="VALUE_CONSTRAINT_VIOLATED"):
                await _add(
                    client,
                    base,
                    '<https://example.org/data/bad> rdf:type ex:Asset ; ex:sku "bad" '
                    ".\n",
                )
            owned = ", ".join(f"<https://example.org/data/a{n}>" for n in range(1, 5))
            with pytest.raises(EngineResponseError, match="LINK_CARDINALITY_EXCEEDED"):
                await _add(
                    client,
                    base,
                    '<https://example.org/data/p1> rdf:type ex:Person ; ex:name "Ann" '
                    "; "
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


@pytest.mark.spec("MEDULLA-TYPED-PACKS-R650.5")
def test_refusal_carries_rule_path_and_value_digest() -> None:
    async def exercise() -> None:
        tag = uuid.uuid4().hex[:8]
        connector = f"object-model-{tag}"
        base = f"https://example.org/{connector}"
        graph = f"object-model-{tag}"
        receipts: list[dict] = []
        client = await _import_and_attach(connector, base, graph, receipts)
        try:
            # The import receipt carries the digest of the compiled shapes.
            digest = receipts[0]["object_model_shape_digest"]
            assert len(str(digest)) == 64, receipts[0]

            secret = "bad-offending-value"
            with pytest.raises(EngineResponseError) as refused:
                await _add(
                    client,
                    base,
                    f"<https://example.org/data/x> rdf:type ex:Asset ; ex:sku "
                    f'"{secret}" .\n',
                )
            text = str(refused.value)
            assert "VALUE_CONSTRAINT_VIOLATED" in text, text
            assert "/shape/" in text, text  # the rule: the compiled shape IRI
            assert f"{base}#sku" in text, text  # the property path
            expected = hashlib.sha256(f'"{secret}"'.encode()).hexdigest()
            assert expected in text, text  # the digest of the offending value
            assert secret not in text, text  # never the raw value
        finally:
            await client.tenants.delete(graph)
            await client.close()

    asyncio.run(exercise())


@pytest.mark.spec("MEDULLA-TYPED-PACKS-R650.6")
def test_pack_value_link_interface_declarations_are_enforced_on_write() -> None:
    async def exercise() -> None:
        tag = uuid.uuid4().hex[:8]
        connector = f"object-model-{tag}"
        base = f"https://example.org/{connector}"
        graph = f"object-model-{tag}"
        other_graph = f"object-model-other-{tag}"
        client = await _import_and_attach(connector, base, graph)
        try:
            ok_asset = (
                '<https://example.org/data/a1> rdf:type ex:Asset ; ex:sku "ABC-1" .\n'
            )
            await _add(client, base, ok_asset)

            # Each violating write refuses; the valid one committed alone.
            with pytest.raises(EngineResponseError, match="VALUE_CONSTRAINT_VIOLATED"):
                await _add(
                    client,
                    base,
                    '<https://example.org/data/a2> rdf:type ex:Asset ; ex:sku "nope" '
                    ".\n",
                )
            await _add(
                client,
                base,
                '<https://example.org/data/a3> rdf:type ex:Asset ; ex:sku "ABC-3" .\n'
                '<https://example.org/data/a4> rdf:type ex:Asset ; ex:sku "ABC-4" .\n'
                '<https://example.org/data/a5> rdf:type ex:Asset ; ex:sku "ABC-5" .\n',
            )
            owned = ", ".join(f"<https://example.org/data/a{n}>" for n in (1, 3, 4, 5))
            with pytest.raises(EngineResponseError, match="LINK_CARDINALITY_EXCEEDED"):
                await _add(
                    client,
                    base,
                    '<https://example.org/data/p1> rdf:type ex:Person ; ex:name "Ann" '
                    "; "
                    f"ex:owns {owned} .\n",
                )

            # The served query sees the valid data and none of the refused writes.
            found = await client.rdf.get_rdf()
            assert "data/a1" in found and "data/a3" in found
            assert "data/a2" not in found and "data/p1" not in found

            # A second graph that never attached the pack is unaffected.
            await client.tenants.create(other_graph)
            other = await MedullaClient.connect(
                socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
                verified_context=request_context(),
                graph_name=other_graph,
            )
            try:
                await _add(
                    other,
                    base,
                    '<https://example.org/data/a2> rdf:type ex:Asset ; ex:sku "nope" '
                    ".\n",
                )
            finally:
                await other.tenants.delete(other_graph)
                await other.close()
        finally:
            await client.tenants.delete(graph)
            await client.close()

    asyncio.run(exercise())
