"""Served crosswalk acceptance (MEDULLA-TYPED-PACKS-R071.1.4, R071.1.5, R071.1.6).

Drives authenticated ``ConnectorPack.Import`` of ``crosswalk`` entries through
the hand-written Python client and a real server, then ``GraphSchema``
attachment and ``OntologyInspect`` / ``GraphSchemaClasses`` readback.
"""

from __future__ import annotations

import asyncio
import copy
import sys
import uuid
from pathlib import Path
from typing import Any

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from _commons_generation import connect as connect_engine
from _commons_generation import restartable_engine
from ontology_pack_support import (
    FIXTURE_SHA256,
    attach_pack,
    connect,
    core_revision,
    crosswalk_entry,
    crosswalk_record,
    import_entries,
    inspect_committed,
    load_domain_fixture,
    load_fixture,
    ontology_entry,
    violation_codes,
)

from conftest import TEST_TENANT  # isort: skip
from medulla import ConnectorPackClient
from medulla.generated import reasoning
from medulla.generated.connector_pack import PackHeadRef

CORE = "https://knuckles-team.github.io/ontology/core#"
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
@pytest.mark.spec("MEDULLA-TYPED-PACKS-R097.2.3")
def test_core_metadata_crosswalks() -> None:
    fixture = load_fixture()
    assert FIXTURE_SHA256 == (
        "58b059a142a0f55278171e8b51c8d89fad4dbf656a97afa8d13b0d2d65be8bfc"
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
            # The activation's coverage is reported, bound to the imported revision.
            report = next(
                c for c in view.coverage if c.source_id == f"pack:{connector}"
            )
            assert report.source_revision == revision
            assert set(report.mapped) == {source for source, _, _ in GOLDEN}
            assert report.declared == len(report.mapped) + len(report.unmapped)
            assert all(u.class_iri not in report.mapped for u in report.unmapped)
            assert all(u.reason == "no_declared_mapping" for u in report.unmapped)
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
@pytest.mark.spec("MEDULLA-TYPED-PACKS-R097.2.4")
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
                "CROSSWALK_REVISION_MISMATCH",
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
                        "    rdfs:subClassOf "
                        "<https://knuckles-team.github.io/ontology/core#Person> .\n",
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


async def _pack_head(client, connector: str):
    status = await ConnectorPackClient(client).status(
        tenant_id=TEST_TENANT, connector=connector
    )
    assert status.head is not None
    return status.head


@pytest.mark.spec("MEDULLA-TYPED-PACKS-R097.2.4")
def test_refused_domain_import_preserves_heads_and_digests() -> None:
    fixture = load_fixture()

    async def exercise() -> None:
        client = await connect()
        try:
            revision = await core_revision(client)
            connector = _connector("xwalk-forged")
            imported = await import_entries(
                client, connector, _entries(connector, fixture, revision)
            )
            assert imported["result"] == "imported", imported
            composed = await attach_pack(client, connector)
            head = await _pack_head(client, connector)
            state = await _state(client)
            assert state[0] == composed

            # An edited origin no longer matches the record digest: a forged record.
            forged = crosswalk_record(fixture["mappings"][0], fixture, revision)
            forged["origin"] = forged["origin"] + "-edited"
            refused = await import_entries(
                client,
                connector,
                [crosswalk_entry(connector, "forged", forged)],
                expected_head=PackHeadRef(
                    binding_revision=head.binding_revision, pack_digest=head.pack_digest
                ),
            )
            assert "CROSSWALK_EVIDENCE_INVALID" in violation_codes(refused), refused

            # Nothing moved: the pack head, the status and the composed schema digest.
            after = await _pack_head(client, connector)
            assert after.binding_revision == head.binding_revision
            assert str(after.pack_digest) == str(head.pack_digest)
            assert await _state(client) == state
        finally:
            await client.close()

    asyncio.run(exercise())


@pytest.mark.spec("MEDULLA-TYPED-PACKS-R097.2.5")
def test_coverage_matches_pack_status_and_schema_reads() -> None:
    fixture = load_fixture()

    async def exercise() -> None:
        client = await connect()
        try:
            revision = await core_revision(client)
            connector = _connector("xwalk-bound")
            imported = await import_entries(
                client, connector, _entries(connector, fixture, revision)
            )
            assert imported["result"] == "imported", imported
            composed = await attach_pack(client, connector)
            head = await _pack_head(client, connector)

            view = await inspect_committed(client)
            report = next(
                c for c in view.coverage if c.source_id == f"pack:{connector}"
            )
            # The binding names the importer, the immutable pack revision and the
            # vocabulary revisions of the committed crosswalks.
            assert report.tenant_id == TEST_TENANT
            assert report.graph
            assert report.pack_record_id == head.record_id
            assert report.target_vocabulary_revisions == [
                fixture["target_vocabulary_revision"]
            ]
            # The schema digest is the composed digest GraphSchema reads for the graph.
            classes = await reasoning.send_graph_schema_classes(
                client, {"kind": "class", "cursor": None, "limit": 1}
            )
            assert report.schema_digest == classes.composed_digest == composed
        finally:
            await client.close()

    asyncio.run(exercise())


DOMAINS = ("medical", "technology", "labor")


def _domain_entries(connector: str, fixture, revision: str, **override):
    return [
        crosswalk_entry(
            connector,
            mapping["name"],
            crosswalk_record(mapping, fixture, revision, **override),
        )
        for mapping in fixture["mappings"]
    ]


async def _committed_classes(client) -> set[str]:
    """Every committed class IRI, paged by cursor until exhausted."""
    found: set[str] = set()
    cursor = None
    while True:
        page = await reasoning.send_graph_schema_classes(
            client, {"kind": "class", "cursor": cursor, "limit": 500}
        )
        found.update(term.iri for term in page.terms)
        cursor = getattr(page, "next_cursor", None)
        if not cursor:
            return found


# proves R097.2.6: Crosswalk coverage hot-path proof
@pytest.mark.spec("MEDULLA-TYPED-PACKS-R097.2.6")
@pytest.mark.no_engine
def test_domain_coverage_and_refusals(tmp_path: Path) -> None:
    fixtures = {name: load_domain_fixture(name) for name in DOMAINS}
    medical = fixtures["medical"]
    patient = medical["mappings"][0]

    async def refuse(client, revision, mapping, code, **override) -> None:
        connector = _connector("xwalk-domain-refuse")
        before = await _state(client)
        refused = await import_entries(
            client,
            connector,
            [
                crosswalk_entry(
                    connector,
                    "m",
                    crosswalk_record(mapping, medical, revision, **override),
                )
            ],
        )
        assert code in violation_codes(refused), refused
        assert await _state(client) == before

    async def run_one(engine) -> tuple[str, dict]:
        client = await connect_engine(engine=engine)
        try:
            revision = (await inspect_committed(client)).composed_digest
            reports: dict[str, Any] = {}
            for name in DOMAINS:
                fixture = fixtures[name]
                connector = _connector(f"xwalk-{name}")
                imported = await import_entries(
                    client, connector, _domain_entries(connector, fixture, revision)
                )
                assert imported["result"] == "imported", imported
                await attach_pack(client, connector)
                view = await inspect_committed(client)
                report = next(
                    c for c in view.coverage if c.source_id == f"pack:{connector}"
                )
                expect = fixture["expect"]
                assert report.source_revision == revision
                assert list(report.mapped) == expect["mapped"]
                unmapped = {u.class_iri: u.reason for u in report.unmapped}
                for iri in expect["unmapped_sample"]:
                    assert unmapped[iri] == "no_declared_mapping"
                assert not set(report.mapped) & set(unmapped)
                assert [u.class_iri for u in report.unmapped] == sorted(unmapped)
                assert report.declared == len(report.mapped) + len(report.unmapped)
                served = {
                    (c.source_class_iri, c.target_class_iri, c.relation)
                    for c in view.crosswalks
                }
                for mapping in fixture["mappings"]:
                    assert (
                        mapping["source_class_iri"],
                        mapping["target_class_iri"],
                        mapping["relation"],
                    ) in served
                assert set(report.mapped) <= await _committed_classes(client)
                reports[name] = report
            assert len({r.declared for r in reports.values()}) == 1
            assert len({r.source_digest for r in reports.values()}) == 1

            # Each refusal is one change on medical records and moves nothing.
            unknown = copy.deepcopy(patient)
            unknown["source_class_iri"] = f"{CORE}NoSuchClass"
            await refuse(client, revision, unknown, "CROSSWALK_SOURCE_UNKNOWN")

            connector = _connector("xwalk-domain-scope")
            local = copy.deepcopy(patient)
            local["source_class_iri"] = "urn:xwalk:Local"
            before = await _state(client)
            refused = await import_entries(
                client,
                connector,
                [
                    ontology_entry(
                        connector,
                        "local",
                        "@prefix owl: <http://www.w3.org/2002/07/owl#> .\n"
                        "@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n"
                        "<urn:xwalk:Local> a owl:Class ;\n"
                        f"    rdfs:subClassOf <{CORE}Person> .\n",
                    ),
                    crosswalk_entry(
                        connector, "m", crosswalk_record(local, medical, revision)
                    ),
                ],
            )
            assert "CROSSWALK_SOURCE_OUT_OF_SCOPE" in violation_codes(refused)
            assert await _state(client) == before

            await refuse(
                client,
                revision,
                patient,
                "CROSSWALK_REVISION_MISMATCH",
                source_revision="stale-revision",
            )
            await refuse(
                client,
                revision,
                patient,
                "CROSSWALK_TARGET_UNKNOWN",
                approved_target_declarations=[],
            )
            wrong_category = copy.deepcopy(patient)
            wrong_category["target_category"] = "process"
            await refuse(
                client, revision, wrong_category, "CROSSWALK_CATEGORY_CONFLICT"
            )
            broad = copy.deepcopy(patient)
            broad["relation"] = "skos_broad_match"
            await refuse(client, revision, broad, "CROSSWALK_CONFLICT")

            # An origin edited after the digest was computed is forged evidence.
            forged = crosswalk_record(patient, medical, revision)
            forged["origin"] = forged["origin"] + "-edited"
            connector = _connector("xwalk-domain-forged")
            before = await _state(client)
            refused = await import_entries(
                client, connector, [crosswalk_entry(connector, "m", forged)]
            )
            assert "CROSSWALK_EVIDENCE_INVALID" in violation_codes(refused)
            assert await _state(client) == before
            return (await _state(client))[0], {
                name: report.model_dump() for name, report in reports.items()
            }
        finally:
            await client.close()

    async def run_two(engine, digest: str, reports: dict) -> None:
        client = await connect_engine(engine=engine)
        try:
            view = await inspect_committed(client)
            assert view.composed_digest == digest
            for name in DOMAINS:
                served = next(
                    c
                    for c in view.coverage
                    if c.source_id.startswith(f"pack:xwalk-{name}-")
                )
                assert served.model_dump() == reports[name]
            committed = await _committed_classes(client)
            for name in DOMAINS:
                assert set(reports[name]["mapped"]) <= committed
        finally:
            await client.close()

    with restartable_engine(tmp_path, "run1", bootstrap=True) as engine:
        digest, reports = asyncio.run(run_one(engine))
    with restartable_engine(tmp_path, "run2", bootstrap=False) as engine:
        asyncio.run(run_two(engine, digest, reports))
