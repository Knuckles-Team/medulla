"""Hard retrieval constraints, served (MEDULLA-FEDERATED-QUERY-R040.1.4).

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
