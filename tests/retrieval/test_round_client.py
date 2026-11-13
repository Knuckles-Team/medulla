"""Served retrieval round client (MEDULLA-FEDERATED-QUERY-R081.11).

`retrieval_round` runs one plan over the caller's graph and returns the rows
beside one verdict per requested part. The request is the Rust serde shape.
"""

from __future__ import annotations

import os
import uuid
from typing import Any

import msgpack
import pytest
from conftest import request_context


def _client() -> Any:
    from medulla.client import SyncMedullaClient

    return SyncMedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
    )


def _node(client: Any, graph: str, node_id: str) -> None:
    client._send(
        "AddNode",
        {
            "node_id": node_id,
            "properties_msgpack": msgpack.packb({"type": "Doc", "text": node_id}),
        },
        graph,
    )


def _round(client: Any, graph: str, request: dict[str, Any]) -> dict[str, Any]:
    result = client._send("RetrievalRound", {"request": request}, graph)
    if isinstance(result, (bytes, bytearray)):
        result = msgpack.unpackb(result, raw=False)
    return result


def _request(**overrides: Any) -> dict[str, Any]:
    request: dict[str, Any] = {
        "plan": {"ops": [{"Scan": {"label": "Doc"}}]},
        "corpus_revision": "corpus-1",
        "model_revision": "model-1",
    }
    request.update(overrides)
    return request


@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R081.11")
def test_round_request_round_trips(start_medulla_server) -> None:
    """Verifies MEDULLA-FEDERATED-QUERY-R081.11: rows with verdict, tenant scoped."""
    graph = f"round-{uuid.uuid4().hex[:8]}"
    other = f"round-{uuid.uuid4().hex[:8]}"
    client = _client()
    client.tenants.create(graph)
    client.tenants.create(other)
    try:
        for n in range(3):
            _node(client, graph, f"doc-{n}")
        bare = _round(client, graph, _request())
        assert sorted(row["id"] for row in bare["rows"]) == ["doc-0", "doc-1", "doc-2"]
        assert "sufficiency" not in bare

        part = {
            "round": 1,
            "round_limit": 4,
            "k": 4,
            "fallback_mode": "extractive",
            "fallback": "lenient",
        }
        judged = _round(client, graph, _request(sufficiency=part))
        assert judged["sufficiency"]["verdict"] == "sufficient"
        assert judged["sufficiency"]["evidence_count"] == 3

        # Another tenant's graph sees none of those rows.
        assert _round(client, other, _request())["rows"] == []

        # A part without a round limit is refused with its typed code.
        with pytest.raises(Exception, match="RETRIEVAL_ROUND_LIMIT_REQUIRED"):
            _round(client, graph, _request(sufficiency={**part, "round_limit": None}))
    finally:
        client.tenants.delete(graph)
        client.tenants.delete(other)
        client.close()
