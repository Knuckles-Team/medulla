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


@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R652.3")
def test_context_compile_reranks_before_boost(start_medulla_server) -> None:
    """Verifies MEDULLA-FEDERATED-QUERY-R652.3: rerank runs after fuse and before
    calibration, cut and selection, and never in Discover's place."""
    graph = f"rerank-order-{uuid.uuid4().hex[:8]}"
    client = _client()
    client.tenants.create(graph)
    try:
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
        evidence = _compile(client, graph, "lighthouse keeper logbook")["evidence"]
        stages = evidence["stages"]
        # The receipt names the stages in the order they ran: the fused list is
        # reranked, the scores calibrated, and only then is the tail cut and the
        # package selected. No stage ahead of the rerank reorders its output.
        assert stages == ["fuse", "rerank", "calibrate", "cut", "select"]
        assert "boost" not in stages
        rerank = evidence["rerank"]
        # The rerank saw the fused order and changed it; the cut and the
        # selection acted on the reranked order.
        assert rerank["input_ranks"] == [0, 1]
        assert rerank["output_ranks"] == [1, 0]
        assert evidence["selected"][0] == "beta"
        # With no champion curve pinned for the scorer, calibration scales
        # min-max and the receipt says so.
        assert rerank.get("calibration_digest") in (None, "")
        assert rerank["fallback"] == "no_calibration"
    finally:
        client.tenants.delete(graph)
        client.close()
