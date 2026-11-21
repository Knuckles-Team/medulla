"""Served topic fusion over stored centroids (MEDULLA-FEDERATED-QUERY-R040.2.5).

The topic request rides on ``UnifiedQuery.retrieval.topic``. The graph declares one
single-modality space through the served ``DeclareEmbeddingSpace`` write, document vectors
are stored with ``AddEmbedding`` and the centroids are ``TopicCentroid`` nodes written with
``AddNode``.
"""

from __future__ import annotations

import re
import uuid
from typing import Any

import msgpack
import pytest
from _round_served import client, put

from medulla.client import EngineResponseError
from medulla.generated.models import RankedRows

SPACE_ID = "pinned-test-space"
REVISION = "1"
DIMENSIONS = 4
_MICROS = 1_000_000


def _space() -> dict[str, Any]:
    return {
        "id": SPACE_ID,
        "revision": REVISION,
        "model_hash": "sha256:w",
        "preprocessing_hash": "sha256:p",
        "dimensions": DIMENSIONS,
        "normalize": False,
        "digest": "pinned-test-space-digest",
    }


def _eq(prop: str, value: str) -> dict[str, Any]:
    return {"operator": "eq", "property": prop, "value": {"Str": value}}


def _ranked(
    cl: Any, graph: str, query: list[float], k: int, retrieval: dict[str, Any]
) -> RankedRows:
    ops = [
        {"Scan": {"label": "Doc"}},
        {"Rank": {"query": query}},
        {"Limit": {"k": k}},
    ]
    result = cl._send(
        "UnifiedQuery",
        {"plan": {"ops": ops, "report_shortfall": True}, "retrieval": retrieval},
        graph,
    )
    if isinstance(result, (bytes, bytearray)):
        result = msgpack.unpackb(result, raw=False)
    return RankedRows.model_validate(result)


def _corpus_revision(cl: Any, graph: str, retrieval: dict[str, Any]) -> int:
    """The graph revision the query reads, named by the stale-corpus refusal."""
    stale = {**retrieval, "topic": {**retrieval["topic"], "corpus_revision": 2**40}}
    with pytest.raises(EngineResponseError) as refused:
        _ranked(cl, graph, [1.0, 0.0, 0.0, 0.0], 1, stale)
    text = str(refused.value)
    assert "TOPIC_CORPUS_STALE" in text
    match = re.search(r"the corpus is at (\d+)", text)
    assert match, text
    return int(match.group(1))


@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R040.2.5")
def test_receipt_reproduces_final_score(start_medulla_server) -> None:
    """Verifies MEDULLA-FEDERATED-QUERY-R040.2.5: every final score is recomputed from the
    receipt's dense and topic parts by the reproduction rule, the entries follow the final
    order, and a candidate the constraints excluded never appears."""
    graph = f"topic-receipt-{uuid.uuid4().hex[:8]}"
    cl = client()
    cl.tenants.create(graph)
    try:
        assert cl.graph.declare_embedding_space(_space(), graph) == "pinned-test-space-digest"
        docs = [
            ("d1", [1.0, 0.0, 0.0, 0.0]),
            ("d2", [0.9, 0.4, 0.0, 0.0]),
            ("d3", [0.2, 1.0, 0.0, 0.0]),
            ("d4", [0.0, 0.1, 1.0, 0.0]),
        ]
        for node_id, embedding in docs:
            put(cl, graph, node_id, {"type": "Doc", "kind": "doc"}, embedding)
        # The nearest vector to the query is excluded by the constraint.
        put(cl, graph, "m1", {"type": "Doc", "kind": "memo"}, [1.0, 0.2, 0.0, 0.0])
        for topic, centroid in (("sports", [1.0, 0.0, 0.0, 0.0]), ("finance", [0.0, 1.0, 0.0, 0.0])):
            put(cl, graph, f"c-{topic}", {"type": "TopicCentroid", "topic": topic, "centroid": centroid})

        retrieval: dict[str, Any] = {
            "constraints": {"graph_revision": 0, "entity_predicates": [_eq("kind", "doc")]},
            "topic": {
                "top_topics": 2,
                "min_weight": 0.0,
                "corpus_revision": 0,
                "space": _space(),
            },
        }
        retrieval["topic"]["corpus_revision"] = _corpus_revision(cl, graph, retrieval)
        answer = _ranked(cl, graph, [1.0, 0.2, 0.0, 0.0], 3, retrieval)

        assert answer.receipt is not None and answer.receipt.topic is not None
        topic = answer.receipt.topic
        assert topic.backend_id == "stored_centroid"
        assert topic.centroid_generation == retrieval["topic"]["corpus_revision"]
        assert topic.space_tuple == f"{SPACE_ID}|{DIMENSIONS}|{REVISION}"
        assert topic.fusion_policy.name == "weighted_topic_dense"
        weight = topic.fusion_policy.weight_micros
        assert 0 <= weight <= _MICROS

        # One entry per row of the final answer, in final order.
        ids = [row[0] for row in answer.rows]
        assert len(ids) == 3
        assert [part.candidate_id for part in topic.component_scores] == ids
        # The excluded candidate never ranks or scores.
        assert "m1" not in ids
        assert "m1" not in {part.candidate_id for part in topic.component_scores}

        # The reproduction rule, in integer arithmetic with truncating division.
        for part in topic.component_scores:
            expected = ((_MICROS - weight) * part.dense_micros + weight * part.topic_micros) // _MICROS
            assert abs(part.final_micros - expected) <= 1, part
        finals = [(-part.final_micros, part.candidate_id) for part in topic.component_scores]
        assert finals == sorted(finals)
        assert {entry.topic_id for entry in topic.responsibilities} <= {"sports", "finance"}
    finally:
        cl.tenants.delete(graph)
        cl.close()
