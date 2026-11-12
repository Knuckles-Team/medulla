"""Served concept path admission (MEDULLA-TYPED-PACKS-R071.100).

Producers submit proposed paths as ``concept_path`` pack entries; the engine's
vocabulary validation owns canonical identity. Refusals name the conflicting
ids and never change a published class IRI.
"""

from __future__ import annotations

import asyncio
import uuid
from typing import Any

import pytest
from ontology_pack_support import (
    concept_path_entry,
    connect,
    core_revision,
    import_entries,
    inspect_committed,
    load_fixture,
    violation_codes,
)

from medulla.generated import reasoning


def _record(path: str, class_iri: str, revision: str, **extra: Any) -> dict[str, Any]:
    return {
        "schema_version": 1,
        "path": path,
        "class_iri": class_iri,
        "vocabulary_revision": revision,
        "aliases": [],
        **extra,
    }


async def _published_iris(client) -> list[str]:
    classes = await reasoning.send_graph_schema_classes(
        client, {"kind": "class", "cursor": None, "limit": 1000}
    )
    return sorted(term.iri for term in classes.terms)


async def _admit(client, revision: str, fixture) -> str:
    connector = f"cpath-{uuid.uuid4().hex[:8]}"
    entries = [
        concept_path_entry(
            connector,
            item["name"],
            _record(item["path"], item["class_iri"], revision),
        )
        for item in fixture["paths"]
    ]
    admitted = await import_entries(client, connector, entries)
    assert admitted["result"] == "imported", admitted
    return connector


async def _refuse(client, records: list[tuple[str, dict[str, Any]]]) -> set[str]:
    connector = f"cpath-bad-{uuid.uuid4().hex[:8]}"
    refused = await import_entries(
        client,
        connector,
        [concept_path_entry(connector, name, record) for name, record in records],
    )
    return violation_codes(refused)


@pytest.mark.spec("MEDULLA-TYPED-PACKS-R071.100")
def test_owner_approved_path_grammar_is_shared() -> None:
    fixture = load_fixture()
    person = fixture["paths"][0]

    async def exercise() -> None:
        client = await connect()
        try:
            revision = await core_revision(client)
            iris_before = await _published_iris(client)
            await _admit(client, revision, fixture)

            # Malformed segments refuse with the one shared grammar.
            for bad in ("", "a//b", "/a", "a/", "a/ b", "a/%41"):
                codes = await _refuse(
                    client,
                    [("p", _record(bad, person["class_iri"], revision))],
                )
                assert "CONCEPT_PATH_MALFORMED" in codes, bad

            # An alias collision with an already claimed path.
            codes = await _refuse(
                client,
                [
                    ("a", _record("Person", person["class_iri"], revision)),
                    (
                        "b",
                        _record(
                            "Event",
                            fixture["paths"][1]["class_iri"],
                            revision,
                            aliases=["Person"],
                        ),
                    ),
                ],
            )
            assert "CONCEPT_PATH_DUPLICATE_ALIAS" in codes

            # A parent path that is not a direct subclass parent.
            codes = await _refuse(
                client,
                [
                    ("a", _record("Event", fixture["paths"][1]["class_iri"], revision)),
                    (
                        "b",
                        _record(
                            "Event/Document", fixture["paths"][2]["class_iri"], revision
                        ),
                    ),
                ],
            )
            assert "CONCEPT_PATH_PARENT_CONFLICT" in codes

            # A stale vocabulary revision.
            codes = await _refuse(
                client,
                [("a", _record("Person", person["class_iri"], "00" * 32))],
            )
            assert "CONCEPT_PATH_STALE_REVISION" in codes

            assert await _published_iris(client) == iris_before
        finally:
            await client.close()

    asyncio.run(exercise())


@pytest.mark.spec("MEDULLA-TYPED-PACKS-R071.100")
def test_duplicate_or_conflicting_paths_refuse_without_iri_changes() -> None:
    fixture = load_fixture()
    person, event = fixture["paths"][0], fixture["paths"][1]

    async def exercise() -> None:
        client = await connect()
        try:
            revision = await core_revision(client)
            iris_before = await _published_iris(client)
            composed_before = (await inspect_committed(client)).composed_digest

            # Two canonical paths for one class.
            codes = await _refuse(
                client,
                [
                    ("a", _record("Person", person["class_iri"], revision)),
                    ("b", _record("People", person["class_iri"], revision)),
                ],
            )
            assert "CONCEPT_PATH_IRI_CONFLICT" in codes

            # One canonical path claimed by two classes.
            codes = await _refuse(
                client,
                [
                    ("a", _record("Person", person["class_iri"], revision)),
                    ("b", _record("Person", event["class_iri"], revision)),
                ],
            )
            assert "CONCEPT_PATH_DUPLICATE_ALIAS" in codes

            # A path naming a class the pinned vocabulary does not have.
            codes = await _refuse(
                client,
                [("a", _record("Ghost", "https://knuckles-team.github.io/ontology/core#Ghost", revision))],
            )
            assert "CONCEPT_PATH_IRI_CONFLICT" in codes

            # Published IRIs and the composed digest are unchanged.
            assert await _published_iris(client) == iris_before
            assert (await inspect_committed(client)).composed_digest == composed_before
        finally:
            await client.close()

    asyncio.run(exercise())
