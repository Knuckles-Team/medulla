"""MEDULLA-UNIFIED-DATA-PLANE-R038.3 served acceptance: one snapshot per source.

Drives ``Method::SparqlVirtual`` over an R2RML mapping of a PostgreSQL source while a
second connection commits an extra row between the two scans. A query with two
fragments of one source needs a single snapshot; a source that cannot give one (a
plain HTTP/JSON source) refuses the query with ``SNAPSHOT_UNAVAILABLE`` before any
request reaches the source.
"""

from __future__ import annotations

import threading
import time

import pytest

from medulla.client import EngineResponseError

MAPPING = """
@prefix rr: <http://www.w3.org/ns/r2rml#> .
@prefix ex: <http://example.org/> .
<http://example.org/map/A> a rr:TriplesMap ;
  rr:logicalTable [ rr:tableName "snap_a" ] ;
  rr:subjectMap [ rr:template "http://example.org/a/{id}" ] ;
  rr:predicateObjectMap [ rr:predicate ex:aName ; rr:objectMap [ rr:column "name" ] ] .
<http://example.org/map/B> a rr:TriplesMap ;
  rr:logicalTable [ rr:tableName "snap_b" ] ;
  rr:subjectMap [ rr:template "http://example.org/b/{id}" ] ;
  rr:predicateObjectMap [ rr:predicate ex:bName ; rr:objectMap [ rr:column "name" ] ] .
"""

QUERY = (
    "PREFIX ex: <http://example.org/> "
    "SELECT ?v WHERE { { ?s ex:aName ?v } UNION { ?s ex:bName ?v } }"
)

SETUP = """
DROP VIEW IF EXISTS snap_a, snap_b;
DROP TABLE IF EXISTS snap_base;
CREATE TABLE snap_base (id int primary key, name text);
INSERT INTO snap_base VALUES (1, 'alpha'), (2, 'beta'), (3, 'gamma');
CREATE VIEW snap_a AS SELECT b.id::text AS id, b.name
  FROM snap_base b, (SELECT 1 FROM pg_sleep(1)) s;
CREATE VIEW snap_b AS SELECT b.id::text AS id, b.name
  FROM snap_base b, (SELECT 1 FROM pg_sleep(1)) s;
"""


@pytest.mark.spec("MEDULLA-UNIFIED-DATA-PLANE-R038.3.6")
def test_fragments_share_one_snapshot(clean_graph, pg_source):
    for statement in [part for part in SETUP.split(";\n") if part.strip()]:
        pg_source.execute(statement)

    def late_writer() -> None:
        time.sleep(0.3)
        pg_source.execute("INSERT INTO snap_base VALUES (4, 'delta')")

    writer = threading.Thread(target=late_writer)
    writer.start()
    rows = clean_graph.rdf.sparql_virtual(
        QUERY,
        MAPPING,
        [],
        external_sources=[
            {"name": "snap_a", "dsn": pg_source.dsn, "table": "snap_a"},
            {"name": "snap_b", "dsn": pg_source.dsn, "table": "snap_b"},
        ],
    )
    writer.join()

    # The oracle is the snapshot at the first fragment: three rows in each source.
    oracle = sorted(["alpha", "beta", "gamma"] * 2)
    assert sorted(row["v"] for row in rows) == oracle
    assert pg_source.execute("SELECT count(*) FROM snap_base") == "4"
    # The engine stores zero source rows.
    assert clean_graph.rdf.sparql("SELECT ?s WHERE { ?s ?p ?o }") == []


def scan(name: str) -> dict[str, object]:
    return {"ForeignScan": {"source": {"Named": {"name": name}}, "join": False}}


@pytest.mark.spec("MEDULLA-UNIFIED-DATA-PLANE-R038.3.4")
def test_snapshot_refusals(clean_graph):
    clean_graph.query.register_foreign_source(
        "no-snapshot-source",
        {
            "HttpJson": {
                "url": "https://source.example.test/rows",
                "json_path": "",
                "field_map": {"id": "id"},
            }
        },
    )
    with pytest.raises(EngineResponseError) as refused:
        clean_graph.query.unified(
            [scan("no-snapshot-source"), scan("no-snapshot-source")]
        )
    assert "SNAPSHOT_UNAVAILABLE" in str(refused.value)
