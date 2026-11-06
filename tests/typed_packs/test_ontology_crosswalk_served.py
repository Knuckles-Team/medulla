"""Served crosswalk acceptance (MEDULLA-TYPED-PACKS-R071.1.4, R071.1.5, R071.1.6).

Drives authenticated ``ConnectorPack.Import`` of ``crosswalk`` entries through
the hand-written Python client and a real server, then ``GraphSchema``
attachment and ``OntologyInspect`` / ``GraphSchemaClasses`` readback.
"""

from __future__ import annotations

import asyncio
import copy
import uuid

import pytest
from ontology_pack_support import (
    FIXTURE_SHA256,
    attach_pack,
    connect,
    core_revision,
    crosswalk_entry,
    crosswalk_record,
    import_entries,
    inspect_committed,
    load_fixture,
    ontology_entry,
    violation_codes,
)

from medulla.generated import reasoning

CORE = "http://knuckles.team/kg#"
GOLDEN = [
    (f"{CORE}Document", "http://schema.org/DigitalDocument", "skos_close_match"),
    (f"{CORE}Event", "http://schema.org/Event", "skos_close_match"),
    (f"{CORE}Person", "http://schema.org/Person", "skos_exact_match"),
]


def _connector(prefix: str) -> str:
    return f"{prefix}-{uuid.uuid4().hex[:8]}"


def _entries(connector: str, fixture, revision: str, **override):
    return [
        crosswalk_entry(
            connector,
            mapping["name"],
            crosswalk_record(mapping, fixture, revision, **override),
        )
        for mapping in fixture["mappings"]
    ]


async def _state(client) -> tuple[str, list[tuple[str, str, str]]]:
    view = await inspect_committed(client)
    return view.composed_digest, sorted(
        (c.source_class_iri, c.target_class_iri, c.relation) for c in view.crosswalks
    )


@pytest.mark.spec("MEDULLA-TYPED-PACKS-R071.1.6")
def test_core_metadata_crosswalks() -> None:
    fixture = load_fixture()
    assert FIXTURE_SHA256 == (
        "6f8c0ef5d84f5f33910addd861b8b4263343d5f741d2520ceb9f5a80ab5eccc6"
    )

    async def exercise() -> None:
        client = await connect()
        try:
            revision = await core_revision(client)
            before = await _state(client)
            connector = _connector("xwalk-core")

            # Unknown and out-of-scope sources refuse even with a valid target.
            unknown = copy.deepcopy(fixture["mappings"][0])
            unknown["source_class_iri"] = f"{CORE}NoSuchClass"
            refused = await import_entries(
                client,
                _connector("xwalk-unknown"),
                [
                    crosswalk_entry(
                        "x", "u", crosswalk_record(unknown, fixture, revision)
                    )
                ],
            )
            assert "CROSSWALK_SOURCE_UNKNOWN" in violation_codes(refused)
            category = copy.deepcopy(fixture["mappings"][0])
            category["target_category"] = "process"
            refused = await import_entries(
                client,
                _connector("xwalk-category"),
                [
                    crosswalk_entry(
                        "x", "c", crosswalk_record(category, fixture, revision)
                    )
                ],
            )
            assert "CROSSWALK_CATEGORY_CONFLICT" in violation_codes(refused)
            assert await _state(client) == before

            imported = await import_entries(
                client, connector, _entries(connector, fixture, revision)
            )
            assert imported["result"] == "imported", imported
            composed = await attach_pack(client, connector)

            digest, crosswalks = await _state(client)
            assert crosswalks == GOLDEN
            assert digest == composed
            view = await inspect_committed(client)
            assert view.committed is True
            assert {c.source_revision for c in view.crosswalks} == {revision}
            classes = await reasoning.send_graph_schema_classes(
                client, {"kind": "class", "cursor": None, "limit": 1000}
            )
            committed_classes = {term.iri for term in classes.terms}
            for source, _, _ in GOLDEN:
                assert source in committed_classes
        finally:
            await client.close()

    asyncio.run(exercise())


@pytest.mark.spec("MEDULLA-TYPED-PACKS-R071.1.4")
def test_crosswalk_refusals_leave_state_unchanged() -> None:
    fixture = load_fixture()
    person, event = fixture["mappings"][0], fixture["mappings"][1]

    async def refuse(client, revision, mapping, code, **override) -> None:
        connector = _connector("xwalk-refuse")
        refused = await import_entries(
            client,
            connector,
            [
                crosswalk_entry(
                    connector,
                    "m",
                    crosswalk_record(mapping, fixture, revision, **override),
                )
            ],
        )
        assert code in violation_codes(refused), refused

    async def exercise() -> None:
        client = await connect()
        try:
            revision = await core_revision(client)
            before = await _state(client)

            property_source = copy.deepcopy(person)
            property_source["source_class_iri"] = f"{CORE}name"
            await refuse(client, revision, property_source, "CROSSWALK_SOURCE_UNKNOWN")

            stale = "00" * 32
            await refuse(
                client,
                revision,
                person,
                "CROSSWALK_SOURCE_UNKNOWN",
                source_revision=stale,
            )

            # A known class outside the selected source set is out of scope.
            connector = _connector("xwalk-scope")
            out_of_scope = copy.deepcopy(person)
            out_of_scope["source_class_iri"] = "http://example.test/pack#Local"
            refused = await import_entries(
                client,
                connector,
                [
                    ontology_entry(
                        connector,
                        "local",
                        "@prefix owl: <http://www.w3.org/2002/07/owl#> .\n"
                        "@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n"
                        "<http://example.test/pack#Local> a owl:Class ;\n"
                        "    rdfs:subClassOf <http://knuckles.team/kg#Person> .\n",
                    ),
                    crosswalk_entry(
                        connector,
                        "m",
                        crosswalk_record(out_of_scope, fixture, revision),
                    ),
                ],
            )
            assert "CROSSWALK_SOURCE_OUT_OF_SCOPE" in violation_codes(refused), refused

            # Person to Event: the target category differs from the source's.
            person_to_event = copy.deepcopy(person)
            person_to_event["target_class_iri"] = event["target_class_iri"]
            person_to_event["target_category"] = event["target_category"]
            await refuse(
                client, revision, person_to_event, "CROSSWALK_CATEGORY_CONFLICT"
            )

            # A target without an approved declaration.
            await refuse(
                client,
                revision,
                person,
                "CROSSWALK_TARGET_UNKNOWN",
                approved_target_declarations=[
                    {
                        "class_iri": "http://schema.org/Other",
                        "category": person["target_category"],
                    }
                ],
            )

            # A conflicting duplicate: the same pair twice.
            connector = _connector("xwalk-dup")
            record = crosswalk_record(person, fixture, revision)
            duplicate = await import_entries(
                client,
                connector,
                [
                    crosswalk_entry(connector, "a", record),
                    crosswalk_entry(connector, "b", record),
                ],
            )
            assert "CROSSWALK_CONFLICT" in violation_codes(duplicate)

            # Every bound refuses without truncation.
            connector = _connector("xwalk-limit")
            long_iri = copy.deepcopy(person)
            long_iri["target_class_iri"] = "http://schema.org/" + "A" * 3000
            limit = await import_entries(
                client,
                connector,
                [
                    crosswalk_entry(
                        connector, "l", crosswalk_record(long_iri, fixture, revision)
                    )
                ],
            )
            assert "CROSSWALK_LIMIT_EXCEEDED" in violation_codes(limit)

            assert await _state(client) == before
        finally:
            await client.close()

    asyncio.run(exercise())


@pytest.mark.spec("MEDULLA-TYPED-PACKS-R071.1.5")
def test_crosswalk_evidence_matches_committed_records() -> None:
    fixture = load_fixture()

    async def exercise() -> None:
        client = await connect()
        try:
            revision = await core_revision(client)
            inline = await reasoning.send_ontology_inspect(
                client,
                {
                    "documents": [
                        "@prefix owl: <http://www.w3.org/2002/07/owl#> .\n"
                        "<urn:x:Thing> a owl:Class .\n"
                    ],
                    "source_ids": [],
                },
            )
            assert inline.committed is False
            assert list(inline.crosswalks) == []

            connector = _connector("xwalk-evidence")
            records = {
                m["name"]: crosswalk_record(m, fixture, revision)
                for m in fixture["mappings"]
            }
            imported = await import_entries(
                client,
                connector,
                [crosswalk_entry(connector, n, r) for n, r in records.items()],
            )
            assert imported["result"] == "imported", imported
            composed = await attach_pack(client, connector)

            view = await inspect_committed(client)
            assert view.committed is True
            assert view.composed_digest == composed
            by_pair = {
                (c.source_class_iri, c.target_class_iri): c for c in view.crosswalks
            }
            for record in records.values():
                served = by_pair[
                    (record["source_class_iri"], record["target_class_iri"])
                ]
                assert served.relation == record["relation"]
                assert served.source_revision == record["source_revision"]
                assert (
                    served.target_vocabulary_revision
                    == record["target_vocabulary_revision"]
                )
                assert served.origin == record["origin"]
                assert served.crosswalk_digest == record["content_digest"]
                assert len(served.crosswalk_digest) == 64
                assert served.source_id == f"pack:{connector}"
            # Independent readback of the committed classes the records name.
            classes = await reasoning.send_graph_schema_classes(
                client, {"kind": "class", "cursor": None, "limit": 1000}
            )
            assert classes.composed_digest == composed
            assert {r["source_class_iri"] for r in records.values()} <= {
                term.iri for term in classes.terms
            }
        finally:
            await client.close()

    asyncio.run(exercise())
