"""Served retrieval feedback (MEDULLA-FEDERATED-QUERY-R705.3).

An explicit upvote is stored as one `RetrievalFeedback` node stamped with the verified
tenant and principal, and `Discover` adds the decayed boost after calibration, so a
document that tied on its keyword score ranks first once it is upvoted.
"""

from __future__ import annotations

import uuid
from typing import Any

import msgpack
import pytest
from _round_served import client, put


def _discover(cl: Any, graph: str) -> list[str]:
    result = cl._send(
        "Discover",
        {"keywords": ["feedback"], "query_embedding": [], "k": 5},
        graph,
    )
    if isinstance(result, (bytes, bytearray)):
        result = msgpack.unpackb(result, raw=False)
    return [hit["id"] for hit in result["hits"]]


@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R705.3")
def test_upvote_raises_rank(start_medulla_server) -> None:
    graph = f"fb-{uuid.uuid4().hex[:8]}"
    cl = client()
    cl.tenants.create(graph)
    try:
        for node_id in ("doc-a", "doc-b"):
            put(cl, graph, node_id, {"type": "Doc", "name": "feedback", "description": ""})
        before = _discover(cl, graph)
        assert before[:2] == ["doc-a", "doc-b"]

        recorded = cl._send(
            "RetrievalFeedbackRecord",
            {"document_ref": "doc-b", "kind": "upvote", "weight": 0.2},
            graph,
        )
        if isinstance(recorded, (bytes, bytearray)):
            recorded = msgpack.unpackb(recorded, raw=False)
        assert recorded["kind"] == "upvote"
        assert recorded["document_ref"] == "doc-b"

        assert _discover(cl, graph)[:2] == ["doc-b", "doc-a"]

        # Implicit kinds are derived, never recorded.
        with pytest.raises(Exception, match="implicit feedback kinds"):
            cl._send(
                "RetrievalFeedbackRecord",
                {"document_ref": "doc-b", "kind": "cited", "weight": 0.05},
                graph,
            )
    finally:
        cl.tenants.delete(graph)


def _stored_events(cl: Any, graph: str) -> int:
    rows = cl._send(
        "GetNodesByLabel",
        {"label": "RetrievalFeedback", "after": None, "limit": 0},
        graph,
    )
    if isinstance(rows, (bytes, bytearray)):
        rows = msgpack.unpackb(rows, raw=False)
    return len(rows)


@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R705.4")
def test_feedback_refusals(start_medulla_server) -> None:
    """Verifies MEDULLA-FEDERATED-QUERY-R705.4: both refusal codes, and zero writes."""
    graph = f"fb-refuse-{uuid.uuid4().hex[:8]}"
    cl = client()
    cl.tenants.create(graph)
    try:
        put(cl, graph, "doc-a", {"type": "Doc", "name": "feedback", "description": ""})

        # A document the principal cannot read (here: one that does not exist for it)
        # is refused with FEEDBACK_TARGET_HIDDEN and nothing is stored.
        with pytest.raises(Exception, match="FEEDBACK_TARGET_HIDDEN"):
            cl._send(
                "RetrievalFeedbackRecord",
                {"document_ref": "ghost", "kind": "upvote", "weight": 0.2},
                graph,
            )
        assert _stored_events(cl, graph) == 0

        # `feedback.rate_limit_per_hour` is Fixed at 100: the 101st event refuses.
        for _ in range(100):
            cl._send(
                "RetrievalFeedbackRecord",
                {"document_ref": "doc-a", "kind": "upvote", "weight": 0.01},
                graph,
            )
        assert _stored_events(cl, graph) == 100
        with pytest.raises(Exception, match="FEEDBACK_RATE_LIMIT"):
            cl._send(
                "RetrievalFeedbackRecord",
                {"document_ref": "doc-a", "kind": "upvote", "weight": 0.01},
                graph,
            )
        assert _stored_events(cl, graph) == 100
    finally:
        cl.tenants.delete(graph)
        cl.close()


def _discover_hits(cl: Any, graph: str) -> dict[str, dict[str, Any]]:
    result = cl._send(
        "Discover",
        {"keywords": ["feedback"], "query_embedding": [], "k": 5},
        graph,
    )
    if isinstance(result, (bytes, bytearray)):
        result = msgpack.unpackb(result, raw=False)
    return {hit["id"]: hit for hit in result["hits"]}


@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R705.5")
def test_boost_listed_per_hit(start_medulla_server) -> None:
    """Verifies MEDULLA-FEDERATED-QUERY-R705.5: a rank answer lists `boost` per hit and
    the explicit event is stored as a `RetrievalFeedback` node."""
    graph = f"fb-evidence-{uuid.uuid4().hex[:8]}"
    cl = client()
    cl.tenants.create(graph)
    try:
        for node_id in ("doc-a", "doc-b"):
            put(cl, graph, node_id, {"type": "Doc", "name": "feedback", "description": ""})
        before = _discover_hits(cl, graph)
        assert all(hit.get("boost", 0.0) == 0.0 for hit in before.values())

        cl._send(
            "RetrievalFeedbackRecord",
            {"document_ref": "doc-b", "kind": "upvote", "weight": 0.2},
            graph,
        )
        after = _discover_hits(cl, graph)
        assert after["doc-b"]["boost"] == pytest.approx(0.2, rel=0.05)
        assert after["doc-a"].get("boost", 0.0) == 0.0

        stored = cl._send(
            "GetNodesByLabel",
            {"label": "RetrievalFeedback", "after": None, "limit": 0},
            graph,
        )
        if isinstance(stored, (bytes, bytearray)):
            stored = msgpack.unpackb(stored, raw=False)
        assert len(stored) == 1
    finally:
        cl.tenants.delete(graph)
        cl.close()
