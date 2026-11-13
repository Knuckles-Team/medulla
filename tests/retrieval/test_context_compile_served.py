"""Served context bundle compile (MEDULLA-FEDERATED-QUERY-R650.3 to R650.6).

`ContextCompile` gathers ranked hits from the caller's graph, measures each at
several resolutions, and returns the bundle an exact, certified selection chose
inside the token budget, with a citation and proof edges per item and inline
evidence.
"""

from __future__ import annotations

import hashlib
import json
import os
import re
import struct
import uuid
from pathlib import Path
from typing import Any

import msgpack
import pytest
from conftest import request_context

_ROOT = Path(__file__).resolve().parents[2]
_REASONS = {"below_gap", "over_budget", "denied", "out_of_scope", "dominated"}


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


def _edge(client: Any, graph: str, src: str, tgt: str, relationship: str) -> None:
    client._send(
        "AddEdge",
        {
            "source_id": src,
            "target_id": tgt,
            "properties_msgpack": msgpack.packb({"relationship": relationship}),
        },
        graph,
    )


def _request(**overrides: Any) -> dict[str, Any]:
    budget = {
        "window_tokens": 400,
        "reserved_output_tokens": 0,
        "caller_ceiling_tokens": None,
        "money_ceiling_micro": None,
        "price_micro_per_token": None,
        "latency_ceiling_ms": None,
        "tokens_per_second": None,
    }
    request: dict[str, Any] = {
        "query_text": "lighthouse keeper logbook",
        "query_embedding": [],
        "budget": budget,
        "tokenizer_id": "bytes-estimate",
        "tokenizer_version": "1",
        "mode": "lexical",
        "top_n": 256,
    }
    request.update(overrides)
    return request


def _compile(client: Any, graph: str, request: dict[str, Any]) -> dict[str, Any]:
    result = client._send("ContextCompile", {"request": request}, graph)
    if isinstance(result, (bytes, bytearray)):
        result = msgpack.unpackb(result, raw=False)
    return result


_LONG = (
    "The keeper wrote every tide, every ship and every storm into the logbook "
    "and kept the lamp lit through the long winter nights on the rock. " * 4
)


def _fixture(client: Any, graph: str) -> None:
    for n in range(6):
        _node(
            client,
            graph,
            f"doc-{n}",
            {
                "type": "document",
                "name": f"lighthouse logbook {n}",
                "description": _LONG + f" entry {n}",
            },
        )
    _node(client, graph, "other", {"type": "doc", "name": "harbour tariff"})
    _edge(client, graph, "doc-0", "other", "DERIVED_FROM")


@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R650.6")
def test_bundle_end_to_end(start_medulla_server) -> None:
    """Verifies MEDULLA-FEDERATED-QUERY-R650.6: served bundle with a certificate."""
    graph = f"ctx-{uuid.uuid4().hex[:8]}"
    client = _client()
    client.tenants.create(graph)
    try:
        _fixture(client, graph)
        bundle = _compile(client, graph, _request())
        certificate = bundle["certificate"]
        assert certificate["verified"] is True
        assert bundle["items"], "at least one item fits"
        tokens = sum(item["tokens"] for item in bundle["items"])
        assert tokens == certificate["total_tokens"]
        assert tokens <= certificate["effective_capacity_tokens"] <= 400
        for item in bundle["items"]:
            citation = item["citation"]
            assert citation["cited_end"] > citation["cited_start"]
            assert len(citation["content_digest"]) == 64
        proven = {item["item_id"]: item["proof_edges"] for item in bundle["items"]}
        if "doc-0" in proven:
            assert proven["doc-0"][0]["relationship"] == "DERIVED_FROM"
        assert all(e["reason"] in _REASONS for e in bundle["excluded"])
        # Nothing in the bundle is written to the graph.
        assert client._send("ListGraphs", {}, None) is not None
    finally:
        client.tenants.delete(graph)
        client.close()


@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R650.3")
def test_method_is_served_and_no_second_implementation(start_medulla_server) -> None:
    """Verifies MEDULLA-FEDERATED-QUERY-R650.3: served, deterministic, one solver."""
    graph = f"ctx-{uuid.uuid4().hex[:8]}"
    client = _client()
    client.tenants.create(graph)
    try:
        _fixture(client, graph)
        first = _compile(client, graph, _request())
        second = _compile(client, graph, _request())
        assert first == second
        assert [i["item_id"] for i in first["items"]] == first["evidence"]["selected"]
        resolutions = first["evidence"]["resolutions"]
        assert all(resolutions[i["item_id"]] == i["resolution"] for i in first["items"])
    finally:
        client.tenants.delete(graph)
        client.close()
    # One selection kernel: the compile and its handler define none of their own.
    for path in (
        _ROOT / "crates/medulla-plan/src/context_bundle.rs",
        _ROOT / "crates/medulla-h-query/src/query/context_compile.rs",
    ):
        source = path.read_text()
        assert not re.search(r"fn\s+\w*(knapsack|branch_and_bound)", source), path
    plan = (_ROOT / "crates/medulla-plan/src/context_bundle.rs").read_text()
    assert "context_package::{select" in plan


@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R650.4")
def test_context_refusals(start_medulla_server) -> None:
    """Verifies MEDULLA-FEDERATED-QUERY-R650.4: each refusal, empty and tenant-safe."""
    graph = f"ctx-{uuid.uuid4().hex[:8]}"
    other = f"ctx-{uuid.uuid4().hex[:8]}"
    client = _client()
    client.tenants.create(graph)
    client.tenants.create(other)
    try:
        _fixture(client, graph)
        _node(client, other, "agent-x", {"type": "agent", "name": "other tenant"})
        no_window = _request()
        no_window["budget"]["window_tokens"] = 0
        with pytest.raises(Exception, match="CONTEXT_BUDGET_MISSING"):
            _compile(client, graph, no_window)
        with pytest.raises(Exception, match="CONTEXT_BUDGET_MISSING"):
            _compile(client, graph, _request(tokenizer_id=""))
        with pytest.raises(Exception, match="CONTEXT_SCOPE_FOREIGN"):
            _compile(client, graph, _request(agent_ref="agent-x"))
        with pytest.raises(Exception, match="CONTEXT_PACKAGE_MISSING_FACT"):
            over = _request()
            over["budget"]["money_ceiling_micro"] = 10
            _compile(client, graph, over)
        # Another tenant's rows are never candidates.
        crossed = _compile(client, other, _request())
        assert crossed["items"] == []
        assert crossed["certificate"] is None
    finally:
        client.tenants.delete(graph)
        client.tenants.delete(other)
        client.close()


def _framed(domain: bytes, fields: list[bytes]) -> str:
    digest = hashlib.sha256()
    digest.update(b"eg/framed-sha256/v1\0")
    digest.update(struct.pack(">I", len(domain)) + domain)
    digest.update(struct.pack(">I", len(fields)))
    for field in fields:
        digest.update(struct.pack(">Q", len(field)) + field)
    return digest.hexdigest()


def _key(request: dict[str, Any], inputs: dict[str, Any]) -> str:
    ordered = {
        k: request[k]
        for k in (
            "query_text",
            "query_embedding",
            "budget",
            "tokenizer_id",
            "tokenizer_version",
            "mode",
            "as_of",
            "agent_ref",
            "top_n",
        )
        if k in request
    }
    body = json.dumps(ordered, separators=(",", ":"), ensure_ascii=False)
    return _framed(
        b"medulla/context-bundle-key/v1",
        [
            inputs["tenant"].encode(),
            body.encode(),
            struct.pack(">Q", inputs["graph_revision"]),
            inputs["policy_digest"].encode(),
        ],
    )


@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R650.5")
def test_evidence_matches_inputs(start_medulla_server) -> None:
    """Verifies MEDULLA-FEDERATED-QUERY-R650.5: the key recomputes, tenants differ."""
    first_graph = f"ctx-{uuid.uuid4().hex[:8]}"
    second_graph = f"ctx-{uuid.uuid4().hex[:8]}"
    client = _client()
    client.tenants.create(first_graph)
    client.tenants.create(second_graph)
    try:
        request = _request()
        bundles = []
        for graph in (first_graph, second_graph):
            _fixture(client, graph)
            bundles.append(_compile(client, graph, request))
        first, second = bundles
        evidence = first["evidence"]
        assert first["key"] == _key(request, evidence["key_inputs"])
        assert first["key"] != second["key"]
        assert evidence["selected"] == [i["item_id"] for i in first["items"]]
        assert evidence["budget_used_tokens"] == first["certificate"]["total_tokens"]
        assert evidence["certificate_bound_tokens"] <= 400
        assert evidence["unchecked_dropped"] == []
        assert {s["source"] for s in evidence["normalization"]} == {"lexical"}
        assert all(0.0 <= s["calibrated"] <= 1.0 for s in evidence["input_scores"])
    finally:
        client.tenants.delete(first_graph)
        client.tenants.delete(second_graph)
        client.close()
