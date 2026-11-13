"""Served rerank stage of ContextCompile (MEDULLA-FEDERATED-QUERY-R652.7).

The server binds its rerank model from configuration; the session engine binds
the deterministic lexical-overlap model. `ContextCompile` fuses the arms, then
reranks the head of the fused list through that model, and the bundle evidence
names the model it scored with.
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


def _node(client: Any, graph: str, node_id: str, props: dict[str, Any]) -> None:
    client._send(
        "AddNode",
        {"node_id": node_id, "properties_msgpack": msgpack.packb(props)},
        graph,
    )


def _compile(client: Any, graph: str, query: str) -> dict[str, Any]:
    request = {
        "query_text": query,
        "query_embedding": [],
        "budget": {
            "window_tokens": 400,
            "reserved_output_tokens": 0,
            "caller_ceiling_tokens": None,
            "money_ceiling_micro": None,
            "price_micro_per_token": None,
            "latency_ceiling_ms": None,
            "tokens_per_second": None,
        },
        "tokenizer_id": "bytes-estimate",
        "tokenizer_version": "1",
        "mode": "lexical",
        "top_n": 64,
    }
    result = client._send("ContextCompile", {"request": request}, graph)
    if isinstance(result, (bytes, bytearray)):
        result = msgpack.unpackb(result, raw=False)
    return result


@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R652.7")
def test_model_scorer_when_artifact_bound(start_medulla_server) -> None:
    """Verifies MEDULLA-FEDERATED-QUERY-R652.7: the bound model reranks."""
    graph = f"rerank-{uuid.uuid4().hex[:8]}"
    client = _client()
    client.tenants.create(graph)
    try:
        # The keyword arm ranks alpha (two of three query words in its name) above
        # beta (one word); the model also sees beta's text and scores it higher.
        _node(
            client,
            graph,
            "alpha",
            {"type": "document", "name": "lighthouse keeper", "description": "notes"},
        )
        _node(
            client,
            graph,
            "beta",
            {
                "type": "document",
                "name": "lighthouse",
                "description": "notes",
                "text": "keeper logbook",
            },
        )
        bundle = _compile(client, graph, "lighthouse keeper logbook")
        rerank = bundle["evidence"]["rerank"]
        assert rerank["scorer_name"] == "model_cross_encoder"
        assert rerank["scorer_version"] == "builtin:lexical-overlap@1"
        assert rerank["fallback"] != "scorer_unavailable"
        # Inputs arrive alpha, beta; the model puts beta first.
        assert rerank["input_ranks"] == [0, 1]
        assert rerank["output_ranks"] == [1, 0]
        assert bundle["evidence"]["selected"][0] == "beta"
    finally:
        client.tenants.delete(graph)
        client.close()
