"""Served catalog asset write guard (MEDULLA-TYPED-PACKS-R108.4).

Writes catalog triples through the real served ``AddTriples`` surface. A node
typed with a ``cat:`` class that is not under ``cat:Asset`` is refused with
``ASSET_CLASS_ORPHAN``. A node with no owner commits flagged ``owner_missing``.
A person field the writer holds no marking for is refused with ``FIELD_MARKED``
and the same field commits for a writer whose scopes carry ``marking:pii``. A
refused write changes nothing.
"""

from __future__ import annotations

import os
import uuid

import pytest
from conftest import request_context

from medulla.client import SyncMedullaClient

CATALOG = "https://knuckles-team.github.io/ontology/catalog#"
PREFIX = f"@prefix cat: <{CATALOG}> .\n"
SHAPES = (
    "@prefix sh: <http://www.w3.org/ns/shacl#> .\n"
    "@prefix ex: <http://example.test/r1084#> .\n"
    "ex:UnusedShape a sh:NodeShape ; sh:targetClass ex:UnusedClass .\n"
)


def _connect(graph: str, scopes: list[str] | None) -> SyncMedullaClient:
    return SyncMedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        graph_name=graph,
        verified_context=request_context(scopes=scopes),
    )


@pytest.fixture
def catalog_graph():
    """Give this served test a graph isolated from xdist peers."""

    graph = f"medulla-r1084-{uuid.uuid4().hex}"
    admin = _connect(graph, None)
    created = False
    try:
        admin.tenants.create(graph, graph_type="Global")
        created = True
        admin.rdf.icv_configure(SHAPES)
        yield graph, admin
    finally:
        try:
            if created:
                admin.tenants.delete(graph)
        finally:
            admin.close()


def _rows(client, subject: str, predicate: str) -> list[str]:
    rows = client.rdf.sparql(f"SELECT ?o WHERE {{ <{subject}> <{predicate}> ?o }}")
    return [row["o"] for row in rows]


@pytest.mark.spec("MEDULLA-TYPED-PACKS-R108.4")
def test_orphan_class_missing_owner_and_marked_field_are_handled(catalog_graph):
    # spec: MEDULLA-TYPED-PACKS-R108.4
    graph, admin = catalog_graph
    rdf_type = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type"

    # An orphan class is refused with its code and nothing is written.
    orphan = "urn:medulla:asset:product:orphan"
    with pytest.raises(Exception, match="ASSET_CLASS_ORPHAN"):
        admin.rdf.add_triples(turtle=f"{PREFIX}<{orphan}> a cat:Widget .\n")
    assert _rows(admin, orphan, rdf_type) == []

    # A node with no owner commits flagged owner_missing.
    ownerless = "urn:medulla:asset:product:ownerless"
    report = admin.rdf.add_triples(turtle=f"{PREFIX}<{ownerless}> a cat:Product .\n")
    assert report["triples"] >= 2
    assert _rows(admin, ownerless, f"{CATALOG}flag") == ["owner_missing"]

    # A node that names an owner is not flagged.
    owned = "urn:medulla:asset:product:owned"
    admin.rdf.add_triples(
        turtle=(
            f"{PREFIX}<{owned}> a cat:Product ; "
            "cat:hasOwner <urn:principal:data-team> .\n"
        )
    )
    assert _rows(admin, owned, f"{CATALOG}flag") == []

    # A marked person field is refused for a writer that holds no marking, even
    # one holding the wildcard scope, and the person is not written.
    person = "urn:medulla:asset:person:alice"
    email = f'{PREFIX}<{person}> a cat:PersonAsset ; cat:email "alice@example.test" .\n'
    with pytest.raises(Exception, match="FIELD_MARKED"):
        admin.rdf.add_triples(turtle=email)
    assert _rows(admin, person, f"{CATALOG}email") == []

    # The same write commits for a writer whose scopes carry the marking.
    cleared = _connect(graph, ["kg:read", "kg:write", "marking:pii"])
    try:
        cleared.rdf.add_triples(turtle=email)
    finally:
        cleared.close()
    assert _rows(admin, person, f"{CATALOG}email") == ["alice@example.test"]
