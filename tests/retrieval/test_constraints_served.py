"""Hard retrieval constraints, served (MEDULLA-FEDERATED-QUERY-R040.1.3, R040.1.4).

The constraints ride on ``UnifiedQuery.retrieval``. A malformed predicate and a
graph-revision pin that is not the revision the query reads each refuse with a
registered code, before anything is ranked.
"""

from __future__ import annotations

import uuid
from typing import Any

import msgpack
import pytest
from _round_served import client, put

from medulla.client import EngineResponseError

_SCAN = [{"Scan": {"label": "Doc"}}, {"Rank": {"query": [1.0, 0.0]}}, {"Limit": {"k": 5}}]


def _unified(cl: Any, graph: str, constraints: dict[str, Any]) -> Any:
    result = cl._send(
        "UnifiedQuery",
        {"plan": {"ops": _SCAN}, "retrieval": {"constraints": constraints}},
        graph,
    )
    if isinstance(result, (bytes, bytearray)):
        result = msgpack.unpackb(result, raw=False)
    return result


def _eq(prop: str, value: str) -> dict[str, Any]:
    return {"operator": "eq", "property": prop, "value": {"Str": value}}


@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R040.1.4")
def test_refusals_rank_nothing(start_medulla_server) -> None:
    """Verifies MEDULLA-FEDERATED-QUERY-R040.1.4: a refused constraint ranks nothing."""
    graph = f"constraints-{uuid.uuid4().hex[:8]}"
    cl = client()
    cl.tenants.create(graph)
    try:
        put(cl, graph, "d1", {"type": "Doc", "kind": "doc"}, [1.0, 0.0])
        put(cl, graph, "d2", {"type": "Doc", "kind": "memo"}, [0.0, 1.0])

        # A malformed predicate is CONSTRAINT_INVALID and ranks nothing.
        with pytest.raises(EngineResponseError) as invalid:
            _unified(cl, graph, {"graph_revision": 0, "entity_predicates": [_eq("", "x")]})
        assert "CONSTRAINT_INVALID" in str(invalid.value)

        # A revision pin that is not the revision the query reads is stale.
        with pytest.raises(EngineResponseError) as stale:
            _unified(
                cl,
                graph,
                {"graph_revision": 2**40, "entity_predicates": [_eq("kind", "doc")]},
            )
        assert "GRAPH_REVISION_STALE" in str(stale.value)

        # Revision 0 declares no pin: the same predicates are accepted.
        accepted = _unified(
            cl, graph, {"graph_revision": 0, "entity_predicates": [_eq("kind", "doc")]}
        )
        assert isinstance(accepted, list)
    finally:
        cl.tenants.delete(graph)
        cl.close()


def _ranked(cl: Any, graph: str, ops: list[dict[str, Any]], constraints: dict[str, Any]) -> Any:
    result = cl._send(
        "UnifiedQuery",
        {
            "plan": {"ops": ops, "report_shortfall": True},
            "retrieval": {"constraints": constraints},
        },
        graph,
    )
    if isinstance(result, (bytes, bytearray)):
        result = msgpack.unpackb(result, raw=False)
    return result


@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R040.1.3")
def test_optimized_and_unoptimized_plans_agree(start_medulla_server) -> None:
    """Verifies MEDULLA-FEDERATED-QUERY-R040.1.3: the constrained set feeds the rank
    step whichever order the plan is written in."""
    graph = f"constraints-plans-{uuid.uuid4().hex[:8]}"
    cl = client()
    cl.tenants.create(graph)
    try:
        rows = [
            ("d1", "doc", 2025.0, [0.9, 0.1]),
            ("d2", "doc", 2025.0, [0.8, 0.2]),
            ("d3", "doc", 2025.0, [0.7, 0.3]),
            ("d4", "doc", 2020.0, [0.6, 0.4]),
            # The nearest vectors are excluded by the constraint.
            ("d5", "memo", 2025.0, [1.0, 0.0]),
            ("d6", "memo", 2025.0, [0.99, 0.01]),
        ]
        for node_id, kind, year, embedding in rows:
            put(cl, graph, node_id, {"type": "Doc", "kind": kind, "year": year}, embedding)
        constraints = {"graph_revision": 0, "entity_predicates": [_eq("kind", "doc")]}
        recent = {"Filter": {"preds": [{"GtNum": {"prop": "year", "n": 2024.0}}]}}
        scan, rank, limit = _SCAN[0], _SCAN[1], {"Limit": {"k": 10}}
        # Filter before the rank (a top-k step) and the rank before the filter (the
        # whole order): the constrained set reaches both.
        filter_first = _ranked(cl, graph, [scan, recent, rank, limit], constraints)
        rank_first = _ranked(cl, graph, [scan, rank, recent, limit], constraints)
        ids = lambda answer: [row[0] for row in answer["rows"]]  # noqa: E731
        assert ids(filter_first) == ["d1", "d2", "d3"]
        assert ids(rank_first) == ids(filter_first)
        # A hard exclusion never becomes a score penalty: d5 and d6 never appear.
        everything = _ranked(cl, graph, [scan, rank, limit], constraints)
        assert ids(everything) == ["d1", "d2", "d3", "d4"]
    finally:
        cl.tenants.delete(graph)
        cl.close()


@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R040.1.4")
def test_unsatisfiable_constraints_return_eligible_exhausted(start_medulla_server) -> None:
    """Verifies MEDULLA-FEDERATED-QUERY-R040.1.4: a valid conjunction nothing satisfies
    answers zero rows and the R039 shortfall."""
    graph = f"constraints-none-{uuid.uuid4().hex[:8]}"
    cl = client()
    cl.tenants.create(graph)
    try:
        put(cl, graph, "d1", {"type": "Doc", "kind": "doc"}, [1.0, 0.0])
        constraints = {
            "graph_revision": 0,
            "entity_predicates": [_eq("kind", "doc"), _eq("kind", "memo")],
        }
        answer = _ranked(cl, graph, _SCAN, constraints)
        assert answer["rows"] == []
        assert answer["shortfall"] == {
            "requested": 5,
            "found": 0,
            "reason": "eligible_exhausted",
        }
    finally:
        cl.tenants.delete(graph)
        cl.close()


@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R040.1.5")
def test_excluded_never_reappears(start_medulla_server) -> None:
    """Verifies MEDULLA-FEDERATED-QUERY-R040.1.5: the receipt carries the constraint binding,
    identical across plan shapes, and no excluded candidate appears in any ranking branch."""
    graph = f"constraints-receipt-{uuid.uuid4().hex[:8]}"
    cl = client()
    cl.tenants.create(graph)
    try:
        rows = [
            ("d1", "doc", [0.9, 0.1]),
            ("d2", "doc", [0.8, 0.2]),
            ("d3", "doc", [0.7, 0.3]),
            ("m1", "memo", [1.0, 0.0]),
            ("m2", "memo", [0.99, 0.01]),
        ]
        for node_id, kind, embedding in rows:
            put(cl, graph, node_id, {"type": "Doc", "kind": kind}, embedding)
        constraints = {"graph_revision": 0, "entity_predicates": [_eq("kind", "doc")]}
        scan, rank = _SCAN[0], _SCAN[1]
        top_k = _ranked(cl, graph, [scan, rank, {"Limit": {"k": 2}}], constraints)
        whole = _ranked(cl, graph, [scan, rank, {"Limit": {"k": 10}}], constraints)
        for answer in (top_k, whole):
            assert not {row[0] for row in answer["rows"]} & {"m1", "m2"}
        traces = [answer["receipt"]["constraint"] for answer in (top_k, whole)]
        # The same binding for every branch: digest, revision and the two exclusions.
        assert traces[0] == traces[1]
        assert len(traces[0]["constraint_digest"]) == 64
        assert traces[0]["graph_revision"] == 0
        assert traces[0]["authorized_exclusion_count"] == 2
        assert traces[0]["per_predicate"] == [{"predicate_id": "entity[0]", "outcome": 3}]
    finally:
        cl.tenants.delete(graph)
        cl.close()


def _cites(cl: Any, graph: str, source: str, target: str) -> None:
    cl._send(
        "AddEdge",
        {
            "source_id": source,
            "target_id": target,
            "properties_msgpack": msgpack.packb({"relationship": "cites"}),
        },
        graph,
    )


def _cites_kind(kind: str, quantifier: str) -> dict[str, Any]:
    return {
        "edge_kind": "cites",
        "direction": "Out",
        "target": _eq("kind", kind),
        "quantifier": quantifier,
    }


@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R040.1.6")
def test_constraints_before_ranking(start_medulla_server) -> None:
    """Verifies MEDULLA-FEDERATED-QUERY-R040.1.6: required and forbidden edge predicates,
    a contradictory conjunction, exact top-k replenishment, a stale revision and a
    cross-tenant negative, over edges loaded through the wire."""
    graph = f"constraints-hot-{uuid.uuid4().hex[:8]}"
    other = f"constraints-other-{uuid.uuid4().hex[:8]}"
    cl = client()
    cl.tenants.create(graph)
    cl.tenants.create(other)
    try:
        docs = [
            ("d1", [1.0, 0.0], "bad"),
            ("d2", [0.9, 0.1], "good"),
            ("d3", [0.8, 0.2], "good"),
            ("d4", [0.7, 0.3], None),
        ]
        put(cl, graph, "src-bad", {"type": "Source", "kind": "bad"})
        put(cl, graph, "src-good", {"type": "Source", "kind": "good"})
        for node_id, embedding, cited in docs:
            put(cl, graph, node_id, {"type": "Doc", "kind": "doc"}, embedding)
            if cited is not None:
                _cites(cl, graph, node_id, f"src-{cited}")
        # Another tenant's nearest document must never surface here.
        put(cl, other, "foreign", {"type": "Doc", "kind": "doc"}, [1.0, 0.0])

        ids = lambda answer: [row[0] for row in answer["rows"]]  # noqa: E731
        doc = _eq("kind", "doc")
        scan, rank = _SCAN[0], _SCAN[1]

        # Forbidden edge: d1 cites the banned source, so it never ranks although it is nearest.
        forbidden = {
            "graph_revision": 0,
            "entity_predicates": [doc],
            "relationship_predicates": [_cites_kind("bad", "not_exists")],
        }
        top_two = _ranked(cl, graph, [scan, rank, {"Limit": {"k": 2}}], forbidden)
        # Exact top-k replenishment: two rows survive, the nearest eligible ones.
        assert ids(top_two) == ["d2", "d3"]
        assert top_two["shortfall"] is None
        everything = _ranked(cl, graph, [scan, rank, {"Limit": {"k": 10}}], forbidden)
        assert ids(everything) == ["d2", "d3", "d4"]

        # Required edge: only documents that cite the good source.
        required = {
            "graph_revision": 0,
            "entity_predicates": [doc],
            "relationship_predicates": [_cites_kind("good", "exists")],
        }
        assert ids(_ranked(cl, graph, [scan, rank, {"Limit": {"k": 10}}], required)) == [
            "d2",
            "d3",
        ]

        # A contradictory conjunction ranks nothing and says why.
        contradictory = {"graph_revision": 0, "entity_predicates": [doc, _eq("kind", "memo")]}
        none = _ranked(cl, graph, [scan, rank, {"Limit": {"k": 3}}], contradictory)
        assert none["rows"] == []
        assert none["shortfall"]["reason"] == "eligible_exhausted"

        # A stale pin refuses before anything is ranked.
        with pytest.raises(EngineResponseError) as stale:
            _ranked(
                cl,
                graph,
                [scan, rank, {"Limit": {"k": 3}}],
                {"graph_revision": 2**40, "entity_predicates": [doc]},
            )
        assert "GRAPH_REVISION_STALE" in str(stale.value)

        # Cross-tenant: the other tenant's nearest document is absent from every answer.
        for answer in (top_two, everything):
            assert "foreign" not in ids(answer)
    finally:
        cl.tenants.delete(graph)
        cl.tenants.delete(other)
        cl.close()
