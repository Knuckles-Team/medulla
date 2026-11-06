"""MEDULLA-FEDERATED-QUERY-R709 served acceptance: GeoSPARQL query functions.

Drives ``Method::Sparql`` against the session engine over inserted
``geo:wktLiteral`` points: a served ``geof:sfWithin`` FILTER (R709.3), the
coded operand refusals ``GEO_LITERAL_INVALID`` / ``GEO_CRS_UNSUPPORTED``
(R709.4), and the EXPLAIN evidence naming each spatial function and whether
a spatial index served it (R709.5).
"""

from __future__ import annotations

import pytest

from medulla.client import EngineResponseError

GEO = "http://www.opengis.net/ont/geosparql#"
WKT = f"<{GEO}wktLiteral>"
AS_WKT = f"<{GEO}asWKT>"
EPSG_4326 = "http://www.opengis.net/def/crs/EPSG/0/4326"
PREFIXES = (
    f"PREFIX geo: <{GEO}>\n"
    "PREFIX geof: <http://www.opengis.net/def/function/geosparql/>\n"
)
SQUARE = "POLYGON((0 0, 10 0, 10 10, 0 10, 0 0))"


def seed_points(client) -> None:
    """Three points: two inside the unit square ``SQUARE``, one outside; one
    of the inside points names the EPSG:4326 coordinate reference."""
    client.rdf.add_triples(
        ntriples=(
            f'<http://ex/a> {AS_WKT} "POINT(1 1)"^^{WKT} .\n'
            f'<http://ex/b> {AS_WKT} "<{EPSG_4326}> POINT(5 5)"^^{WKT} .\n'
            f'<http://ex/c> {AS_WKT} "POINT(20 20)"^^{WKT} .\n'
        )
    )


def within_query(polygon: str) -> str:
    return (
        PREFIXES
        + "SELECT ?f WHERE { ?f geo:asWKT ?w . "
        + f'FILTER(geof:sfWithin(?w, "{polygon}"^^geo:wktLiteral)) }}'
    )


@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R709.3")
def test_within_query(clean_graph):
    seed_points(clean_graph)
    rows = clean_graph.rdf.sparql(within_query(SQUARE))
    assert sorted(row["f"] for row in rows) == ["http://ex/a", "http://ex/b"]


@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R709.4")
def test_geo_refusals(clean_graph):
    seed_points(clean_graph)
    with pytest.raises(EngineResponseError) as malformed:
        clean_graph.rdf.sparql(within_query("POLYGON((0 0, 10"))
    assert malformed.value.code == "GEO_LITERAL_INVALID"

    crs84 = f"<http://www.opengis.net/def/crs/OGC/1.3/CRS84> {SQUARE}"
    with pytest.raises(EngineResponseError) as foreign_crs:
        clean_graph.rdf.sparql(within_query(crs84))
    assert foreign_crs.value.code == "GEO_CRS_UNSUPPORTED"

    # EPSG:4326 named explicitly is the supported reference.
    rows = clean_graph.rdf.sparql(within_query(f"<{EPSG_4326}> {SQUARE}"))
    assert len(rows) == 2


@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R709.5")
def test_explain_names_spatial_index(clean_graph):
    seed_points(clean_graph)
    explained = clean_graph.rdf.sparql_result(within_query(SQUARE), explain=True)
    assert [use.function for use in explained.spatial] == ["sfWithin"]
    plain = clean_graph.rdf.sparql_result(within_query(SQUARE))
    assert plain.spatial == []


# spec: MEDULLA-FEDERATED-QUERY-R709.7
@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R709.7")
def test_explain_reports_index_used(clean_graph):
    """The maintained graph spatial index serves the sfWithin FILTER as an index
    scan (EXPLAIN ``index_used``), with the same rows as the scan, including
    after a write moves a point out of the window."""
    seed_points(clean_graph)
    explained = clean_graph.rdf.sparql_result(within_query(SQUARE), explain=True)
    assert [(use.function, use.index_used) for use in explained.spatial] == [
        ("sfWithin", True)
    ]
    assert sorted(row["f"] for row in clean_graph.rdf.sparql(within_query(SQUARE))) == [
        "http://ex/a",
        "http://ex/b",
    ]
    # sfDisjoint takes the complement of the candidate set.
    disjoint = within_query(SQUARE).replace("sfWithin", "sfDisjoint")
    explained = clean_graph.rdf.sparql_result(disjoint, explain=True)
    assert [(use.function, use.index_used) for use in explained.spatial] == [
        ("sfDisjoint", True)
    ]
    assert [row["f"] for row in clean_graph.rdf.sparql(disjoint)] == ["http://ex/c"]
