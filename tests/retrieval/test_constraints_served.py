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
