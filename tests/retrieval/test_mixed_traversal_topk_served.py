"""Mixed traversal and vector queries return ``k`` or a typed shortfall
(MEDULLA-FEDERATED-QUERY-R039).

A ``Scan`` seed, a two-hop ``Traverse`` and a ``Rank`` then ``Limit k`` run as one served
``UnifiedQuery`` whose plan sets ``report_shortfall``. The fixture has two-hop traversal,
parallel edges with stable edge ids, and ten permitted matches that sit below the ten
unfiltered nearest vectors, so a rank that truncated before the traversal filter would
return none of them.
"""

from __future__ import annotations

import uuid
from typing import Any

import msgpack
import pytest
from _round_served import client, put

from medulla.client import EngineResponseError

_LEAVES = 12


def _edge(cl: Any, graph: str, src: str, tgt: str, edge_id: str) -> None:
    cl._send(
        "AddEdge",
        {
            "source_id": src,
            "target_id": tgt,
            "properties_msgpack": msgpack.packb({"relationship": "LINK", "edge_id": edge_id}),
        },
        graph,
    )


def _ranked(cl: Any, graph: str, ops: list[dict[str, Any]]) -> dict[str, Any]:
    result = cl._send(
        "UnifiedQuery", {"plan": {"ops": ops, "report_shortfall": True}}, graph
    )
    if isinstance(result, (bytes, bytearray)):
        result = msgpack.unpackb(result, raw=False)
    return result


def _plan(k: int) -> list[dict[str, Any]]:
    return [
        {"Scan": {"label": "Seed"}},
        {"Traverse": {"rel": "LINK", "min": 1, "max": 2}},
        {"Rank": {"query": [1.0, 0.0]}},
        {"Limit": {"k": k}},
    ]


@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R039")
def test_k_or_typed_shortfall(start_medulla_server) -> None:
    """Verifies MEDULLA-FEDERATED-QUERY-R039: exactly ``k`` rows, or a named shortfall."""
    graph = f"topk-{uuid.uuid4().hex[:8]}"
    cl = client()
    cl.tenants.create(graph)
    try:
        put(cl, graph, "seed", {"type": "Seed"})
        # Two hop-one nodes without an embedding, each linked by two parallel edges.
        for mid in ("m0", "m1"):
            put(cl, graph, mid, {"type": "Mid"})
            _edge(cl, graph, "seed", mid, f"{mid}-a")
            _edge(cl, graph, "seed", mid, f"{mid}-b")
        # Twelve permitted matches two hops out, far from the query vector.
        for index in range(_LEAVES):
            leaf = f"t{index:02d}"
            put(cl, graph, leaf, {"type": "Leaf"}, [0.0, 1.0 - index * 0.01])
            _edge(cl, graph, f"m{index % 2}", leaf, f"{leaf}-edge")
        # Ten unreachable nodes that are the unfiltered nearest vectors.
        for index in range(10):
            put(cl, graph, f"u{index:02d}", {"type": "Decoy"}, [1.0, 0.0])

        # k permitted matches survive the traversal filter: exactly k rows, no shortfall.
        full = _ranked(cl, graph, _plan(10))
        assert len(full["rows"]) == 10
        assert all(row[0].startswith("t") for row in full["rows"])
        assert full.get("shortfall") is None

        # More than the permitted, embedded matches: a typed shortfall naming the count.
        short = _ranked(cl, graph, _plan(20))
        assert len(short["rows"]) == _LEAVES
        assert short["shortfall"] == {
            "requested": 20,
            "found": _LEAVES,
            "reason": "missing_embedding",
        }

        # Nothing eligible: zero rows and eligible_exhausted.
        none = _ranked(
            cl,
            graph,
            [
                {"Scan": {"label": "Nothing"}},
                {"Rank": {"query": [1.0, 0.0]}},
                {"Limit": {"k": 5}},
            ],
        )
        assert none["rows"] == []
        assert none["shortfall"] == {
            "requested": 5,
            "found": 0,
            "reason": "eligible_exhausted",
        }

        # k = 0 is a malformed request.
        with pytest.raises(EngineResponseError) as refused:
            _ranked(cl, graph, _plan(0))
        assert "RETRIEVAL_K_INVALID" in str(refused.value)
    finally:
        cl.tenants.delete(graph)
        cl.close()
