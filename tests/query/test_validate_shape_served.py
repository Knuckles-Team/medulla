"""The typed consistency-violation matrix over the served UQL wire
(MEDULLA-FEDERATED-QUERY-R005.1).

`VALIDATE SHAPE "integrity:consistency"` reads the axioms of the queried graph's composed
ontology and reports each violation with typed, kind-specific fields. The stage only reads.
"""

from __future__ import annotations

import json
import uuid
from collections.abc import Iterator

import pytest

NS = "http://ex/"
SOURCE_ID = "admin:consistency-matrix"
ONTOLOGY = """
@prefix ex: <http://ex/> .
@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
ex:irr a owl:ObjectProperty, owl:IrreflexiveProperty .
ex:asym a owl:ObjectProperty, owl:AsymmetricProperty .
ex:func a owl:ObjectProperty, owl:FunctionalProperty .
ex:member a owl:ObjectProperty .
ex:Team a owl:Class ; rdfs:subClassOf [ a owl:Restriction ;
    owl:onProperty ex:member ; owl:minCardinality "1"^^xsd:nonNegativeInteger ] .
ex:A a owl:Class ; owl:disjointWith ex:B .
ex:B a owl:Class .
"""


@pytest.fixture
def attached(clean_graph) -> Iterator[None]:
    graph = clean_graph._client._graph_name
    clean_graph._send(
        "GraphSchema",
        {"op": {"op": "attach", "source_id": SOURCE_ID, "ontology_ttl": ONTOLOGY}},
        graph,
        idempotency_key=f"attach-{uuid.uuid4().hex}",
    )
    try:
        yield
    finally:
        clean_graph._send(
            "GraphSchema",
            {"op": {"op": "detach", "source_id": SOURCE_ID}},
            graph,
            idempotency_key=f"detach-{uuid.uuid4().hex}",
        )


def _violations(graph, label: str, shape: str = "integrity:consistency") -> dict[str, list]:
    result = graph.query.uql(
        f'MATCH (:{label}) |> VALIDATE SHAPE "{shape}" PROJECT "{NS}" KEEP VIOLATING '
        "|> LIMIT 100000"
    )
    return {
        row["id"]: json.loads(row["provenance"]["shape.violations"]) for row in result["rows"]
    }


@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R005.1")
def test_violation_matrix_five_kinds(clean_graph, attached) -> None:
    tag = uuid.uuid4().hex[:8]
    label = f"Consistency{tag}"

    def node(name: str, cls: str | None = None) -> str:
        key = f"{name}-{tag}"
        clean_graph.nodes.add(key, {"type": cls or label})
        return key

    # One violating fixture and one repaired control per kind.
    loop, calm, other = node("loop"), node("calm"), node("other")
    a1, b1, a2, b2 = node("a1"), node("b1"), node("a2"), node("b2")
    x, y, t1, t2 = node("x"), node("y"), node("t1"), node("t2")
    empty, full, kid = node("empty", "Team"), node("full", "Team"), node("kid")
    bad, good, both = node("bad", "A"), node("good", "A"), node("both", "A")
    # A node has one `type`; the second class of the defect node rides a typing edge.
    clean_graph.nodes.add(f"classB-{tag}", {"type": "B"})

    def edge(source: str, relationship: str, target: str, **extra: int) -> None:
        clean_graph.edges.add(source, target, {"relationship": relationship, **extra})

    edge(loop, "irr", loop)
    edge(calm, "irr", other)
    edge(a1, "asym", b1)
    edge(b1, "asym", a1)
    edge(a2, "asym", b2)
    edge(x, "func", t1, valid_from=1, valid_until=10)
    edge(x, "func", t2, valid_from=5, valid_until=20)
    edge(y, "func", t1, valid_from=1, valid_until=4)
    edge(y, "func", t2, valid_from=4, valid_until=9)
    edge(full, "member", kid)
    edge(bad, "http://www.w3.org/1999/02/22-rdf-syntax-ns#type", f"classB-{tag}")
    # Two independent defects on one node: a self-loop and disjoint classes.
    edge(both, "irr", both)
    edge(both, "http://www.w3.org/1999/02/22-rdf-syntax-ns#type", f"classB-{tag}")

    nodes_before = clean_graph.query.uql(f"MATCH (:{label}) |> LIMIT 100000")["rows"]
    found: dict[str, list] = {}
    for scan_label in (label, "Team", "A"):
        found.update(_violations(clean_graph, scan_label))

    def kinds(key: str) -> list[str]:
        return sorted(v["detail"]["kind"] for v in found.get(key, []))

    assert kinds(loop) == ["self_loop"]
    assert kinds(a1) == ["asymmetry"]
    assert kinds(x) == ["temporal_conflict"]
    assert kinds(empty) == ["cardinality"]
    assert kinds(bad) == ["ontology_defect"]
    assert kinds(both) == ["ontology_defect", "self_loop"]
    for control in (calm, a2, b1, b2, y, full, good):
        assert kinds(control) == [], control

    asymmetry = found[a1][0]
    assert asymmetry["axiom_id"] == f"asymmetric:{NS}asym"
    assert asymmetry["detail"]["subject"] == min(a1, b1)
    assert asymmetry["ontology_revision"]
    cardinality = found[empty][0]["detail"]
    assert (cardinality["min"], cardinality["observed"]) == (1, 0)

    # Reading is not writing: the node set is unchanged by the validation.
    nodes_after = clean_graph.query.uql(f"MATCH (:{label}) |> LIMIT 100000")["rows"]
    assert {r["id"] for r in nodes_after} == {r["id"] for r in nodes_before}

    # A pinned stale revision refuses before any check.
    with pytest.raises(Exception) as refused:
        _violations(clean_graph, label, f"integrity:consistency:{'0' * 64}")
    assert "ONTOLOGY_REVISION_STALE" in str(refused.value)
