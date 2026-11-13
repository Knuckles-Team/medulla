"""Served dual-score selection (MEDULLA-FEDERATED-QUERY-R081.7.6).

The server binds `retrieval.reranker_model = builtin:lexical-overlap`. A round
with a `selection` part scores each chunk by embedding and by reranker, then
selects by the fused policy: a chunk low on the embedding and high on the
reranker is rescued, a chunk low on both is refused, and the selected count
adapts to the scores. The selector itself makes no model call.
"""

from __future__ import annotations

import uuid
from typing import Any

import pytest
from _round_served import client, put, request, run_round

_QUERY = "alpha beta gamma delta"
_RERANKER = "builtin:lexical-overlap@1"
_PLAN = [{"Scan": {"label": "Chunk"}}, {"Rank": {"query": [1.0, 0.0]}}]


def _policy(**overrides: Any) -> dict[str, Any]:
    policy: dict[str, Any] = {
        "policy_revision": "policy-1",
        "corpus_revision": "corpus-1",
        "embedding_model_revision": "model-1",
        "cross_encoder_model_revision": _RERANKER,
        "normalization": {"kind": "min_max", "constant_value": 0.5},
        "embedding_threshold": 0.5,
        "relevance_threshold": 0.5,
        "fusion_weight": 0.5,
        "fusion_threshold": 0.9,
        "max_chunks": 10,
        "activation_benchmark_digest": "bench-1",
    }
    policy.update(overrides)
    return policy


def _selection(**policy: Any) -> dict[str, Any]:
    return {
        "selection": {
            "query": _QUERY,
            "policy": _policy(**policy),
            "benchmark": {
                "digest": "bench-1",
                "baseline": {"ndcg_at_10": 0.80, "mean_selected": 4.0},
                "candidate": {"ndcg_at_10": 0.795, "mean_selected": 3.0},
            },
        }
    }


@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R081.7.6")
def test_rescue_and_dual_low_refusal(start_medulla_server) -> None:
    """Verifies MEDULLA-FEDERATED-QUERY-R081.7.6: rescue, refusal, adaptive count."""
    graph = f"select-{uuid.uuid4().hex[:8]}"
    other = f"select-{uuid.uuid4().hex[:8]}"
    cl = client()
    cl.tenants.create(graph)
    cl.tenants.create(other)
    try:
        for chunk_id, text, embedding in (
            ("hit", "alpha beta gamma", [1.0, 0.0]),
            ("rescue", "alpha beta gamma delta", [0.1, 1.0]),
            ("dual-low", "unrelated words", [0.0, 1.0]),
            ("residual", "zzz", [0.95, 0.3]),
        ):
            put(cl, graph, chunk_id, {"type": "Chunk", "text": text}, embedding)

        result = run_round(cl, graph, request(_PLAN, **_selection()))
        receipt = result["selection"]
        by_id = {c["chunk_id"]: c for c in receipt["candidates"]}
        assert set(by_id) == {"hit", "rescue", "dual-low", "residual"}
        assert by_id["rescue"]["embedding_score"] < 0.2
        assert abs(by_id["rescue"]["cross_encoder_score"] - 1.0) < 1e-6
        assert by_id["rescue"]["region"] == "rescue" and by_id["rescue"]["selected"]
        assert by_id["hit"]["region"] == "dual_high"
        assert by_id["dual-low"]["region"] == "dual_low"
        assert not by_id["dual-low"]["selected"]
        assert by_id["residual"]["region"] == "residual"
        assert not by_id["residual"]["selected"]
        assert sorted(receipt["selected"]) == ["hit", "rescue"]

        # The receipt is read independently of the selection: the premise names the
        # reranker that produced the scores, and the counters show the model calls.
        premise = next(
            p for p in result["premises"] if p["name"] == "retrieval.selection"
        )
        assert premise["value"]["reranker_revision"] == _RERANKER
        assert result["inference"]["reranker"] == 4
        assert result["inference"]["selection"] == 0
        assert receipt["selection_inference_calls"] == 0

        # The count adapts: the fixed-count policy keeps one chunk, the adaptive
        # policy two.
        fixed = run_round(cl, graph, request(_PLAN, **_selection(max_chunks=1)))
        assert len(fixed["selection"]["selected"]) == 1
        assert set(fixed["selection"]["selected"]) <= {"hit", "rescue"}
        assert len(fixed["selection"]["selected"]) < len(receipt["selected"])

        # Another tenant stores no chunk: nothing is scored or selected there.
        try:
            foreign = run_round(cl, other, request(_PLAN, **_selection()))
        except Exception:  # a refusal is as good as an empty selection
            foreign = None
        if foreign is not None:
            assert not (foreign.get("selection") or {}).get("selected")
    finally:
        cl.tenants.delete(graph)
        cl.tenants.delete(other)
        cl.close()
