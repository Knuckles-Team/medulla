"""Served evidence forest (MEDULLA-FEDERATED-QUERY-R736.4, R736.5).

A round with `context_budget_tokens` expands the plan's rows along the graph's relations,
selects a forest under the budget and returns the seeds and the forest as its evidence rows.
The sufficiency judge reads those rows. Without the field the round keeps the plan's rows.
"""

from __future__ import annotations

import sys
import uuid
from pathlib import Path
from typing import Any

import msgpack
import pytest

_RETRIEVAL = Path(__file__).resolve().parent.parent / "retrieval"
sys.path.insert(0, str(_RETRIEVAL))

from _round_served import client, request, run_round  # noqa: E402

_SEED_PLAN = [{"Scan": {"label": "Seed"}}]


def _node(cl: Any, graph: str, node_id: str, label: str, text: str) -> None:
    cl._send(
        "AddNode",
        {
            "node_id": node_id,
            "properties_msgpack": msgpack.packb({"type": label, "text": text}),
        },
        graph,
    )


def _edge(cl: Any, graph: str, src: str, tgt: str, weight: float) -> None:
    cl._send(
        "AddEdge",
        {
            "source_id": src,
            "target_id": tgt,
            "properties_msgpack": msgpack.packb({"weight": weight}),
        },
        graph,
    )


def _seed_graph(cl: Any, graph: str) -> None:
    _node(cl, graph, "seed-1", "Seed", "the seed memory")
    _node(cl, graph, "fact-1", "Fact", "a supported fact next to the seed")
    _node(cl, graph, "fact-2", "Fact", "a second supported fact")
    _node(cl, graph, "far-1", "Fact", "a fact no relation reaches")
    _edge(cl, graph, "seed-1", "fact-1", 0.9)
    _edge(cl, graph, "seed-1", "fact-2", 0.8)


def _ids(result: dict[str, Any]) -> list[str]:
    return [row["id"] for row in result["rows"]]


@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R736.4")
def test_round_with_budget_returns_forest_rows(start_medulla_server) -> None:
    """A budget returns the seed and its supported neighbours, and not an unreached node."""
    graph = f"forest-{uuid.uuid4().hex[:8]}"
    cl = client()
    cl.tenants.create(graph)
    try:
        _seed_graph(cl, graph)
        plain = run_round(cl, graph, request(_SEED_PLAN))
        assert _ids(plain) == ["seed-1"]
        with_budget = run_round(
            cl, graph, request(_SEED_PLAN, context_budget_tokens=4000)
        )
        assert _ids(with_budget)[0] == "seed-1"
        assert {"fact-1", "fact-2"} <= set(_ids(with_budget))
        assert "far-1" not in _ids(with_budget)
        premise = next(
            p for p in with_budget["premises"] if p["name"] == "retrieval.forest"
        )
        assert premise["value"]["budget_tokens"] == 4000
        assert premise["value"]["tokens_used"] <= 4000
        assert premise["value"]["dropped"] == []
        # The sufficiency judge counts the forest rows, not the plan's one row.
        assert with_budget["sufficiency"]["evidence_count"] == len(with_budget["rows"])
        assert plain["sufficiency"]["evidence_count"] == 1
    finally:
        cl.tenants.delete(graph)


@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R736.4")
def test_round_budget_refusals(start_medulla_server) -> None:
    """A zero budget and a round with no seed refuse with their forest codes."""
    graph = f"forest-{uuid.uuid4().hex[:8]}"
    cl = client()
    cl.tenants.create(graph)
    try:
        _seed_graph(cl, graph)
        with pytest.raises(Exception, match="FOREST_BUDGET_ZERO"):
            run_round(cl, graph, request(_SEED_PLAN, context_budget_tokens=0))
        with pytest.raises(Exception, match="FOREST_NO_SEEDS"):
            run_round(
                cl,
                graph,
                request([{"Scan": {"label": "NoSuchLabel"}}], context_budget_tokens=10),
            )
    finally:
        cl.tenants.delete(graph)


@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R736.4")
def test_budget_below_every_candidate_keeps_only_the_seeds(start_medulla_server) -> None:
    """A one-token budget fits no candidate: the forest is empty and the seeds remain."""
    graph = f"forest-{uuid.uuid4().hex[:8]}"
    cl = client()
    cl.tenants.create(graph)
    try:
        _seed_graph(cl, graph)
        result = run_round(cl, graph, request(_SEED_PLAN, context_budget_tokens=1))
        assert _ids(result) == ["seed-1"]
    finally:
        cl.tenants.delete(graph)
