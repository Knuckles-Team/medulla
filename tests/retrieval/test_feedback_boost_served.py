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
