"""Served web Document citations (MEDULLA-FEDERATED-QUERY-R653.7).

A hybrid hit on a web Document cites its source URL, retrieval time, backend and
originating query, through both served `Discover` and served `ContextCompile`.
A hit on a node that is not a web Document carries none of those fields.
"""

from __future__ import annotations

import uuid
from typing import Any

import pytest
from _round_served import client, hashed, put

_URL = "https://example.org/lighthouse/logbook"
_RETRIEVED_AT = 1_790_000_000
_BACKEND = "metasearch"
_QUERY_DIGEST = "ab" * 32
_TEXT = "lighthouse keeper logbook tides"
_FIELDS = ("url", "retrieved_at", "backend", "query_digest")


def _seed(cl: Any, graph: str) -> None:
    put(
        cl,
        graph,
        "web-doc",
        {
            "type": "Document",
            "name": "lighthouse keeper logbook",
            "description": _TEXT,
            "url": _URL,
            "retrieved_at": _RETRIEVED_AT,
            "backend": _BACKEND,
            "query_digest": _QUERY_DIGEST,
        },
        hashed(_TEXT),
    )
    put(
        cl,
        graph,
        "local-doc",
        {
            "type": "Document",
            "name": "lighthouse keeper notes",
            "description": "lighthouse keeper notes kept locally",
        },
        hashed("lighthouse keeper notes kept locally"),
    )


def _discover(cl: Any, graph: str) -> list[dict[str, Any]]:
    import msgpack

    hits = cl._send(
        "Discover",
        {
            "keywords": ["lighthouse", "keeper"],
            "query_embedding": hashed(_TEXT),
            "k": 5,
            "hops": 0,
        },
        graph,
    )
    if isinstance(hits, (bytes, bytearray)):
        hits = msgpack.unpackb(hits, raw=False)
    return [h if isinstance(h, dict) else h.model_dump() for h in hits]


def _compile(cl: Any, graph: str) -> dict[str, Any]:
    import msgpack

    request = {
        "query_text": "lighthouse keeper logbook",
        "query_embedding": hashed(_TEXT),
        "budget": {
            "window_tokens": 4000,
            "reserved_output_tokens": 0,
            "caller_ceiling_tokens": None,
            "money_ceiling_micro": None,
            "price_micro_per_token": None,
            "latency_ceiling_ms": None,
            "tokens_per_second": None,
        },
        "tokenizer_id": "bytes-estimate",
        "tokenizer_version": "1",
        "mode": "hybrid",
        "top_n": 16,
    }
    result = cl._send("ContextCompile", {"request": request}, graph)
    if isinstance(result, (bytes, bytearray)):
        result = msgpack.unpackb(result, raw=False)
    return result


@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R653.7")
def test_hybrid_hit_cites_source_url(start_medulla_server) -> None:
    """Verifies MEDULLA-FEDERATED-QUERY-R653.7: a web hit cites its source."""
    graph = f"webcite-{uuid.uuid4().hex[:8]}"
    cl = client()
    cl.tenants.create(graph)
    try:
        _seed(cl, graph)

        by_id = {h["id"]: h for h in _discover(cl, graph)}
        web = by_id["web-doc"]
        assert web["url"] == _URL
        assert web["retrieved_at"] == _RETRIEVED_AT
        assert web["backend"] == _BACKEND
        assert web["query_digest"] == _QUERY_DIGEST
        local = by_id["local-doc"]
        assert all(local.get(field) is None for field in _FIELDS)

        bundle = _compile(cl, graph)
        cited = {item["item_id"]: item["citation"] for item in bundle["items"]}
        assert "web-doc" in cited, "the web Document fits the budget"
        citation = cited["web-doc"]
        assert citation["url"] == _URL
        assert citation["retrieved_at"] == _RETRIEVED_AT
        assert citation["backend"] == _BACKEND
        assert citation["query_digest"] == _QUERY_DIGEST
        assert len(citation["content_digest"]) == 64
        if "local-doc" in cited:
            assert all(field not in cited["local-doc"] for field in _FIELDS)
    finally:
        cl.tenants.delete(graph)
        cl.close()
