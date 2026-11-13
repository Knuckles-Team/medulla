"""Served community-hierarchy descent (MEDULLA-FEDERATED-QUERY-R081.9.6).

A round with a community `hierarchy` part descends stored summaries to
communities to entities under a beam, and returns the entities with a receipt of
the cost counters. A stale revision, a dangling entity and an exhausted budget
refuse; another tenant's hierarchy stays unread. The server binds the `hash`
text embedder; the stored vectors come from the same function.
"""

from __future__ import annotations

import uuid
from typing import Any

import pytest
from _round_served import client, hashed, put, request, run_round

_REVISION = "hierarchy-1"
_QUERY = "invoice payment terms"
_NO_ROWS = [{"Scan": {"label": "NoSuchLabel"}}]


def _seed(cl: Any, graph: str) -> None:
    for node_id, text in (
        ("s-root", "company wide overview"),
        ("s-fin", _QUERY),
        ("s-ops", "warehouse shipping logistics"),
    ):
        put(cl, graph, node_id, {"type": "Summary", "text": text}, hashed(text))
    for node_id, text in (
        ("e1", _QUERY),
        ("e2", "refund policy"),
        ("e3", "pallet labels"),
    ):
        put(cl, graph, node_id, {"type": "Entity", "text": text}, hashed(text))
    for node_id, summary, entities in (
        ("c-root", "s-root", []),
        ("c-fin", "s-fin", ["e1", "e2"]),
        ("c-ops", "s-ops", ["e3"]),
    ):
        put(
            cl,
            graph,
            node_id,
            {
                "type": "Community",
                "summary_id": summary,
                "summary_revision": _REVISION,
                "entity_ids": entities,
            },
        )


def _part(**policy: Any) -> dict[str, Any]:
    base: dict[str, Any] = {
        "kind": "community",
        "hierarchy_revision": _REVISION,
        "community_ids": ["c-root", "c-fin", "c-ops"],
        "summary_ids": ["s-root", "s-fin", "s-ops"],
        "entity_ids": ["e1", "e2", "e3"],
        "parent_links": [
            {"parent_id": "c-root", "child_id": "c-fin"},
            {"parent_id": "c-root", "child_id": "c-ops"},
        ],
        "stage": "community",
        "score_provenance": [
            {
                "scorer": "stored-embedding-cosine",
                "scorer_revision": "1",
                "basis": "stored summary embedding",
            }
        ],
        "beam_width": 1,
        "max_depth": 4,
        "max_visited_communities": 10,
        "max_entity_candidates": 10,
        "max_reranker_calls": 20,
        "max_tokens": 1000,
        "max_total_cost_micros": 1000,
        "section_nodes": [],
    }
    base.update(policy)
    return {"hierarchy": {"query": _QUERY, "policy": base, "section": None}}


def _round(cl: Any, graph: str, part: dict[str, Any]) -> dict[str, Any]:
    return run_round(cl, graph, request(_NO_ROWS, **part))


@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R081.9.6")
def test_descent_and_budget_refusals(start_medulla_server) -> None:
    """Verifies MEDULLA-FEDERATED-QUERY-R081.9.6: descent, counters, refusals."""
    graph = f"hier-{uuid.uuid4().hex[:8]}"
    other = f"hier-{uuid.uuid4().hex[:8]}"
    cl = client()
    cl.tenants.create(graph)
    cl.tenants.create(other)
    try:
        _seed(cl, graph)
        result = _round(cl, graph, _part())
        assert result["rows"] == []
        assert result["hierarchy_selected"] == ["e1"]
        assert result["sufficiency"]["evidence_count"] == 1
        assert result["inference"]["embedding"] == 1
        premise = next(
            p for p in result["premises"] if p["name"] == "retrieval.hierarchy"
        )
        receipt = premise["value"]["receipt"]
        stages = [v["stage"] for v in receipt["visits"]]
        assert (
            stages.index("summary") < stages.index("community") < stages.index("entity")
        )
        assert receipt["visited_communities"] == 3
        assert receipt["entity_candidates"] == 2
        assert receipt["reranker_calls"] == 5
        assert receipt["cost"] == 5
        assert receipt["depth"] == 1 and receipt["incomplete"] is False
        visits = {v["id"]: v for v in receipt["visits"]}
        assert visits["c-fin"]["kept"] is True and visits["c-ops"]["kept"] is False
        assert visits["e1"]["kept"] is True and visits["e2"]["kept"] is False
        assert visits["e1"]["path"] == ["c-root", "c-fin"]

        stale = _part(hierarchy_revision="hierarchy-0")
        with pytest.raises(Exception, match="RETRIEVAL_HIERARCHY_INVALID"):
            _round(cl, graph, stale)

        put(
            cl,
            graph,
            "c-ghost",
            {
                "type": "Community",
                "summary_id": "s-root",
                "summary_revision": _REVISION,
                "entity_ids": ["no-such-entity"],
            },
        )
        dangling = _part(
            beam_width=3,
            community_ids=["c-root", "c-fin", "c-ops", "c-ghost"],
            parent_links=[
                {"parent_id": "c-root", "child_id": "c-fin"},
                {"parent_id": "c-root", "child_id": "c-ops"},
                {"parent_id": "c-root", "child_id": "c-ghost"},
            ],
        )
        with pytest.raises(Exception, match="RETRIEVAL_HIERARCHY_INVALID"):
            _round(cl, graph, dangling)

        with pytest.raises(Exception, match="RETRIEVAL_BUDGET_EXCEEDED"):
            _round(cl, graph, _part(max_reranker_calls=2))

        # The second tenant stores none of the hierarchy.
        try:
            foreign = _round(cl, other, _part())
        except Exception:
            foreign = None
        if foreign is not None:
            assert foreign["rows"] == [] and not foreign.get("hierarchy_selected")
    finally:
        cl.tenants.delete(graph)
        cl.tenants.delete(other)
        cl.close()
