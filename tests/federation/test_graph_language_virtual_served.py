"""MEDULLA-UNIFIED-DATA-PLANE-R038.6 served acceptance: openCypher over a graph view.

A named view registered with ``RegisterGraphView`` answers ``CypherQuery`` reads over the
approved R2RML mapping of a PostgreSQL source without copying a row, and
the same question in SPARQL over the same mapping returns the same answer. A view is
owner-scoped: another principal does not resolve it.
"""

from __future__ import annotations

import hashlib
import os

import pytest
from conftest import request_context

from medulla.client import EngineResponseError, SyncMedullaClient

NS = "http://example.org/"

MAPPING = """
@prefix rr: <http://www.w3.org/ns/r2rml#> .
@prefix ex: <http://example.org/> .
<http://example.org/map/Person> a rr:TriplesMap ;
  rr:logicalTable [ rr:tableName "gv_people" ] ;
  rr:subjectMap [ rr:template "http://example.org/person/{id}" ; rr:class ex:Person ] ;
  rr:predicateObjectMap [ rr:predicate ex:name ; rr:objectMap [ rr:column "name" ] ] .
"""

SETUP = [
    "DROP TABLE IF EXISTS gv_people",
    "CREATE TABLE gv_people (id text primary key, name text)",
    "INSERT INTO gv_people VALUES ('1', 'Ann'), ('2', 'Bob'), ('3', 'Cy')",
]

CYPHER = "MATCH (p:Person) RETURN p.name"
SPARQL = (
    "PREFIX ex: <http://example.org/> "
    "SELECT ?name WHERE { ?p a ex:Person . ?p ex:name ?name }"
)


def graph_view() -> dict[str, object]:
    return {
        "mapping_digest": hashlib.sha256(MAPPING.encode()).hexdigest(),
        "node_labels": [
            {
                "label": "Person",
                "ontology_class": f"{NS}Person",
                "mapping_id": "map/Person@1",
            }
        ],
        "relationship_types": [],
    }


def register(client, pg_source) -> None:
    for statement in SETUP:
        pg_source.execute(statement)
    registered = client.query.register_graph_view(
        "people",
        graph_view(),
        MAPPING,
        sources=[{"name": "gv_people", "dsn": pg_source.dsn, "table": "gv_people"}],
    )
    assert registered == "people"


@pytest.mark.spec("MEDULLA-UNIFIED-DATA-PLANE-R038.6.3")
def test_view_is_owner_scoped(clean_graph, pg_source):
    register(clean_graph, pg_source)
    assert sorted(
        row["name"] for row in clean_graph.query.cypher_read(CYPHER, view="people")
    ) == [
        "Ann",
        "Bob",
        "Cy",
    ]

    other = SyncMedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(agent_id="other-agent"),
    )
    with pytest.raises(EngineResponseError) as refused:
        other.query.cypher_read(CYPHER, view="people")
    assert "UNKNOWN_VIEW" in str(refused.value)

    # Without `view` the native graph path runs: the source rows are not there.
    assert clean_graph.query.cypher_read("MATCH (p:Person) RETURN p.name") == []


@pytest.mark.spec("MEDULLA-UNIFIED-DATA-PLANE-R038.6.5")
def test_explain_names_pushed_statement_and_mapping_digest(clean_graph, pg_source):
    register(clean_graph, pg_source)
    rows = clean_graph.query.cypher_read("EXPLAIN " + CYPHER, view="people")
    assert rows, "EXPLAIN returns one row per pushed fragment"
    digest = graph_view()["mapping_digest"]
    for row in rows:
        assert row["mapping_digest"] == digest
        assert len(row["mapping_digest"]) == 64
        assert "gv_people" in row["pushed_statement"]
        assert "postgres://" not in row["pushed_statement"]
        assert f"mapping_digest={digest}" in row["fragment"]


@pytest.mark.spec("MEDULLA-UNIFIED-DATA-PLANE-R038.6.6")
def test_cypher_matches_sparql_over_sql_source(clean_graph, pg_source):
    register(clean_graph, pg_source)
    cypher = sorted(
        row["name"] for row in clean_graph.query.cypher_read(CYPHER, view="people")
    )
    sparql = sorted(
        row["name"]
        for row in clean_graph.rdf.sparql_virtual(
            SPARQL,
            MAPPING,
            [],
            external_sources=[
                {"name": "gv_people", "dsn": pg_source.dsn, "table": "gv_people"}
            ],
        )
    )
    assert cypher == sparql == ["Ann", "Bob", "Cy"]
    # The engine stores zero source rows: nothing was copied for either language.
    assert clean_graph.rdf.sparql("SELECT ?s WHERE { ?s ?p ?o }") == []
    assert clean_graph.query.cypher_read("MATCH (n) RETURN n") == []
