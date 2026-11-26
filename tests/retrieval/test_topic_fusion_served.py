"""Served topic fusion over stored centroids (MEDULLA-FEDERATED-QUERY-R040.2.5,
R040.2.6).

The topic request rides on ``UnifiedQuery.retrieval.topic``. The graph declares one
single-modality space through the served ``DeclareEmbeddingSpace`` write, document
vectors are stored with ``AddEmbedding`` and the centroids are ``TopicCentroid`` nodes
written with ``AddNode``."""

from __future__ import annotations

import uuid
from typing import Any

import msgpack
import pytest
from _round_served import client, put

from medulla.client import EngineResponseError, TopicCorpusStaleError
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
    cl: Any, graph: str, query: list[float], k: int, retrieval: dict[str, Any] | None
) -> RankedRows:
    ops = [
        {"Scan": {"label": "Doc"}},
        {"Rank": {"query": query}},
        {"Limit": {"k": k}},
    ]
    body: dict[str, Any] = {"plan": {"ops": ops, "report_shortfall": True}}
    if retrieval is not None:
        body["retrieval"] = retrieval
    result = cl._send("UnifiedQuery", body, graph)
    if isinstance(result, (bytes, bytearray)):
        result = msgpack.unpackb(result, raw=False)
    return RankedRows.model_validate(result)


def _corpus_revision(cl: Any, graph: str, retrieval: dict[str, Any]) -> int:
    """The graph revision the query reads, a field of the stale-corpus refusal."""
    stale = {**retrieval, "topic": {**retrieval["topic"], "corpus_revision": 2**40}}
    with pytest.raises(TopicCorpusStaleError) as refused:
        _ranked(cl, graph, [1.0, 0.0, 0.0, 0.0], 1, stale)
    assert refused.value.code == "TOPIC_CORPUS_STALE"
    assert refused.value.topics == 2**40
    return refused.value.current


@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R040.2.5")
def test_receipt_reproduces_final_score(start_medulla_server) -> None:
    """Verifies MEDULLA-FEDERATED-QUERY-R040.2.5: every final score is recomputed from
    the receipt's dense and topic parts by the reproduction rule, the entries follow the
    final order, and a candidate the constraints excluded never appears."""
    graph = f"topic-receipt-{uuid.uuid4().hex[:8]}"
    cl = client()
    cl.tenants.create(graph)
    try:
        assert (
            cl.graph.declare_embedding_space(_space(), graph)
            == "pinned-test-space-digest"
        )
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
        for topic_name, centroid in (
            ("sports", [1.0, 0.0, 0.0, 0.0]),
            ("finance", [0.0, 1.0, 0.0, 0.0]),
        ):
            put(
                cl,
                graph,
                f"c-{topic_name}",
                {"type": "TopicCentroid", "topic": topic_name, "centroid": centroid},
            )

        retrieval: dict[str, Any] = {
            "constraints": {
                "graph_revision": 0,
                "entity_predicates": [_eq("kind", "doc")],
            },
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
            expected = (
                (_MICROS - weight) * part.dense_micros + weight * part.topic_micros
            ) // _MICROS
            assert abs(part.final_micros - expected) <= 1, part
        finals = [
            (-part.final_micros, part.candidate_id) for part in topic.component_scores
        ]
        assert finals == sorted(finals)
        assert {entry.topic_id for entry in topic.responsibilities} <= {
            "sports",
            "finance",
        }
    finally:
        cl.tenants.delete(graph)
        cl.close()


def _topic(space: dict[str, Any], corpus_revision: int) -> dict[str, Any]:
    return {
        "retrieval": {
            "topic": {
                "top_topics": 2,
                "min_weight": 0.0,
                "corpus_revision": corpus_revision,
                "space": space,
            }
        }
    }


def _refusal(cl: Any, graph: str, retrieval: dict[str, Any]) -> str:
    with pytest.raises(EngineResponseError) as refused:
        _ranked(cl, graph, [1.0, 0.0, 0.0, 0.0], 3, retrieval["retrieval"])
    return str(refused.value)


@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R040.2.2")
@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R040.2.6")
def test_topic_fusion_over_stored_centroids(start_medulla_server) -> None:
    """Verifies MEDULLA-FEDERATED-QUERY-R040.2.6: topic fusion over stored centroid
    nodes changes the order the dense score alone gives, and an incompatible space, an
    incompatible centroid, a stale corpus revision and another tenant's centroids each
    refuse before anything is ranked."""
    graph = f"topic-hot-{uuid.uuid4().hex[:8]}"
    foreign = f"topic-foreign-{uuid.uuid4().hex[:8]}"
    bare = f"topic-bare-{uuid.uuid4().hex[:8]}"
    cl = client()
    for name in (graph, foreign, bare):
        cl.tenants.create(name)
    try:
        for name in (graph, foreign, bare):
            assert (
                cl.graph.declare_embedding_space(_space(), name)
                == "pinned-test-space-digest"
            )
        # The nearest document by dense score is not the one with the strongest topic.
        docs = [
            ("d1", [0.8, 0.6, 0.0, 0.0]),
            ("d2", [0.78, 0.0, 0.6258, 0.0]),
            ("d3", [0.1, 0.0, 0.0, 0.995]),
        ]
        for node_id, embedding in docs:
            put(cl, graph, node_id, {"type": "Doc"}, embedding)
            put(cl, bare, node_id, {"type": "Doc"}, embedding)
        for topic, centroid in (
            ("a", [1.0, 0.0, 0.0, 0.0]),
            ("b", [0.0, 1.0, 0.0, 0.0]),
        ):
            put(
                cl,
                graph,
                f"c-{topic}",
                {"type": "TopicCentroid", "topic": topic, "centroid": centroid},
            )
        # Another tenant's centroids, of the wrong width.
        put(
            cl,
            foreign,
            "c-x",
            {"type": "TopicCentroid", "topic": "x", "centroid": [1.0, 0.0, 0.0]},
        )

        query = [1.0, 0.0, 0.0, 0.0]
        plain = _ranked(cl, graph, query, 3, None)
        assert [row[0] for row in plain.rows] == ["d1", "d2", "d3"]

        revision = _corpus_revision(cl, graph, _topic(_space(), 2**40)["retrieval"])

        fused = _ranked(cl, graph, query, 3, _topic(_space(), revision)["retrieval"])
        assert [row[0] for row in fused.rows] == ["d2", "d1", "d3"]
        assert fused.receipt is not None and fused.receipt.topic is not None
        assert fused.receipt.topic.backend_id == "stored_centroid"

        # A space pinned to another revision is not the retriever's space.
        other_space = {**_space(), "revision": "2"}
        assert "TOPIC_SPACE_MISMATCH" in _refusal(
            cl, graph, _topic(other_space, revision)
        )
        # A stale corpus revision.
        assert "TOPIC_CORPUS_STALE" in _refusal(
            cl, graph, _topic(_space(), revision + 1)
        )
        # A tenant never reads another tenant's centroids: this graph holds none.
        assert "TOPIC_BACKEND_UNAVAILABLE" in _refusal(
            cl, bare, _topic(_space(), revision)
        )
        # A stored centroid of another width refuses.
        assert "3 dimensions" in _refusal(cl, foreign, _topic(_space(), revision))
    finally:
        for name in (graph, foreign, bare):
            cl.tenants.delete(name)
        cl.close()


def _components(topic: Any) -> dict[str, Any]:
    return {part.candidate_id: part for part in topic.component_scores}


@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R040.2.7")
def test_document_responsibilities_change_order(start_medulla_server) -> None:
    """Verifies MEDULLA-FEDERATED-QUERY-R040.2.7: each topic component is recomputed
    from the receipt's assignments and the query responsibilities; replacing one
    document's embedding changes only that document's component and the order; a missing
    embedding, a duplicate topic, a stale embedding and another space each refuse and
    return no rows."""
    graph = f"topic-docs-{uuid.uuid4().hex[:8]}"
    cl = client()
    cl.tenants.create(graph)
    try:
        assert (
            cl.graph.declare_embedding_space(_space(), graph)
            == "pinned-test-space-digest"
        )
        put(cl, graph, "d1", {"type": "Doc", "kind": "doc"}, [1.0, 0.2, 0.0, 0.0])
        put(cl, graph, "d2", {"type": "Doc", "kind": "doc"}, [0.2, 1.0, 0.0, 0.0])
        put(cl, graph, "m1", {"type": "Doc", "kind": "memo"}, [1.0, 0.2, 0.0, 0.0])
        for topic, centroid in (
            ("a", [1.0, 0.0, 0.0, 0.0]),
            ("b", [0.0, 1.0, 0.0, 0.0]),
        ):
            put(
                cl,
                graph,
                f"c-{topic}",
                {"type": "TopicCentroid", "topic": topic, "centroid": centroid},
            )
        retrieval: dict[str, Any] = {
            "constraints": {
                "graph_revision": 0,
                "entity_predicates": [_eq("kind", "doc")],
            },
            "topic": {
                "top_topics": 2,
                "min_weight": 0.0,
                "corpus_revision": 0,
                "space": _space(),
            },
        }
        retrieval["topic"]["corpus_revision"] = _corpus_revision(cl, graph, retrieval)
        query = [1.0, 0.2, 0.0, 0.0]

        def run() -> Any:
            answer = _ranked(cl, graph, query, 2, retrieval)
            assert answer.receipt is not None and answer.receipt.topic is not None
            return answer, answer.receipt.topic

        first_answer, first = run()
        assert "m1" not in {row[0] for row in first_answer.rows}
        query_weight = {r.topic_id: r.weight_micros for r in first.responsibilities}
        # Assignments: final order, then topic id ascending.
        order = [row[0] for row in first_answer.rows]
        assert [(t.candidate_id, t.topic_id) for t in first.assignments] == [
            (doc, topic) for doc in order for topic in ("a", "b")
        ]
        by_doc = {
            (t.candidate_id, t.topic_id): t.responsibility_micros
            for t in first.assignments
        }
        for part in first.component_scores:
            expected = (
                sum(
                    query_weight[topic] * by_doc[(part.candidate_id, topic)]
                    for topic in query_weight
                )
                / _MICROS
            )
            assert abs(part.topic_micros - expected) <= 2, part

        # Only d2's embedding changes: d1's entry is untouched, d2's topic term and the
        # order change.
        put(cl, graph, "d2", {"type": "Doc", "kind": "doc"}, [1.0, 0.2, 0.0, 0.0])
        retrieval["topic"]["corpus_revision"] = _corpus_revision(cl, graph, retrieval)
        second_answer, second = run()
        # The graph revision moved, so only the fusion parts are compared, not the
        # generation.
        assert (
            _components(second)["d1"].topic_micros
            == _components(first)["d1"].topic_micros
        )
        assert (
            _components(second)["d2"].topic_micros
            != _components(first)["d2"].topic_micros
        )
        assert [row[0] for row in second_answer.rows] != order

        # A stored embedding older than the document's content refuses.
        cl._send(
            "CompareAndSetNodeFields",
            {
                "node_id": "d1",
                "conditions_msgpack": msgpack.packb({"kind": "doc"}),
                "updates_msgpack": msgpack.packb(
                    {"content_revision": 2, "embedded_revision": 1}
                ),
            },
            graph,
        )
        retrieval["topic"]["corpus_revision"] = _corpus_revision(cl, graph, retrieval)
        with pytest.raises(EngineResponseError) as refused:
            _ranked(cl, graph, query, 2, retrieval)
        assert "TOPIC_ASSIGNMENT_STALE" in str(refused.value)

        # A second centroid for topic `a` is a duplicate assignment.
        put(
            cl,
            graph,
            "c-a2",
            {"type": "TopicCentroid", "topic": "a", "centroid": [0.0, 0.0, 1.0, 0.0]},
        )
        retrieval["topic"]["corpus_revision"] = _corpus_revision(cl, graph, retrieval)
        with pytest.raises(EngineResponseError) as duplicate:
            _ranked(cl, graph, query, 2, retrieval)
        assert "TOPIC_ASSIGNMENT_DUPLICATE" in str(duplicate.value)

        # A request space of another revision is a space mismatch.
        retrieval["topic"]["space"] = {**_space(), "revision": "2"}
        with pytest.raises(EngineResponseError) as mismatch:
            _ranked(cl, graph, query, 2, retrieval)
        assert "TOPIC_SPACE_MISMATCH" in str(mismatch.value)
    finally:
        cl.tenants.delete(graph)
        cl.close()


@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R040.2.7")
def test_fused_rank_candidate_without_embedding_refuses(start_medulla_server) -> None:
    """Verifies MEDULLA-FEDERATED-QUERY-R040.2.7: a text-ranked row with no stored
    embedding reaches topic fusion through ``FuseRrf`` and refuses
    ``TOPIC_ASSIGNMENT_MISSING`` without returning rows."""
    graph = f"topic-unembedded-{uuid.uuid4().hex[:8]}"
    cl = client()
    cl.tenants.create(graph)
    try:
        assert (
            cl.graph.declare_embedding_space(_space(), graph)
            == "pinned-test-space-digest"
        )
        put(cl, graph, "d1", {"type": "Doc", "text": "alpha"}, [1.0, 0.2, 0.0, 0.0])
        put(cl, graph, "d2", {"type": "Doc", "text": "alpha"}, [0.2, 1.0, 0.0, 0.0])
        # The text matches this document, which holds no embedding.
        put(cl, graph, "d3", {"type": "Doc", "text": "alpha needle"})
        for topic, centroid in (
            ("a", [1.0, 0.0, 0.0, 0.0]),
            ("b", [0.0, 1.0, 0.0, 0.0]),
        ):
            put(
                cl,
                graph,
                f"c-{topic}",
                {"type": "TopicCentroid", "topic": topic, "centroid": centroid},
            )
        retrieval: dict[str, Any] = {
            "topic": {
                "top_topics": 2,
                "min_weight": 0.0,
                "corpus_revision": 0,
                "space": _space(),
            }
        }
        retrieval["topic"]["corpus_revision"] = _corpus_revision(cl, graph, retrieval)
        query = [1.0, 0.2, 0.0, 0.0]
        plan = {
            "ops": [
                {"Scan": {"label": "Doc"}},
                {
                    "FuseRrf": {
                        "branches": [
                            [{"Rank": {"query": query}}],
                            [{"RankText": {"query": "needle"}}],
                        ],
                        "k": 0.0,
                    }
                },
                {"Limit": {"k": 3}},
            ],
            "report_shortfall": True,
        }
        with pytest.raises(EngineResponseError) as refused:
            cl._send("UnifiedQuery", {"plan": plan, "retrieval": retrieval}, graph)
        assert refused.value.code == "TOPIC_ASSIGNMENT_MISSING"
    finally:
        cl.tenants.delete(graph)
        cl.close()


@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R040.2.4")
def test_incompatible_topics_refuse_before_fusion(start_medulla_server) -> None:
    """Verifies MEDULLA-FEDERATED-QUERY-R040.2.4: a topic weight out of range, a space
    the retriever does not hold, a stale corpus revision and a graph with no centroid
    backend each refuse with their registered code, and none returns rows."""
    graph = f"topic-refuse-{uuid.uuid4().hex[:8]}"
    bare = f"topic-refuse-bare-{uuid.uuid4().hex[:8]}"
    cl = client()
    for name in (graph, bare):
        cl.tenants.create(name)
    try:
        for name in (graph, bare):
            assert (
                cl.graph.declare_embedding_space(_space(), name)
                == "pinned-test-space-digest"
            )
        for node_id, embedding in (
            ("d1", [0.8, 0.6, 0.0, 0.0]),
            ("d2", [0.1, 0.0, 0.0, 0.995]),
        ):
            put(cl, graph, node_id, {"type": "Doc"}, embedding)
            put(cl, bare, node_id, {"type": "Doc"}, embedding)
        put(
            cl,
            graph,
            "c-a",
            {"type": "TopicCentroid", "topic": "a", "centroid": [1.0, 0.0, 0.0, 0.0]},
        )
        revision = _corpus_revision(cl, graph, _topic(_space(), 2**40)["retrieval"])

        # A topic weight bound out of range refuses.
        bad_weight = _topic(_space(), revision)
        bad_weight["retrieval"]["topic"]["min_weight"] = 1.5
        assert "TOPIC_WEIGHT_INVALID" in _refusal(cl, graph, bad_weight)
        # A space the retriever does not hold: equal dimension alone is not compatible.
        other_space = {**_space(), "revision": "2"}
        assert "TOPIC_SPACE_MISMATCH" in _refusal(
            cl, graph, _topic(other_space, revision)
        )
        # A corpus revision the centroids no longer match.
        assert "TOPIC_CORPUS_STALE" in _refusal(
            cl, graph, _topic(_space(), revision + 1)
        )
        # A graph that stores no centroids has no backend.
        assert "TOPIC_BACKEND_UNAVAILABLE" in _refusal(
            cl, bare, _topic(_space(), revision)
        )

        # The compatible request still ranks, so each refusal above came from the topic
        # request.
        ok = _ranked(
            cl, graph, [1.0, 0.0, 0.0, 0.0], 2, _topic(_space(), revision)["retrieval"]
        )
        assert [row[0] for row in ok.rows]
    finally:
        for name in (graph, bare):
            cl.tenants.delete(name)
        cl.close()
