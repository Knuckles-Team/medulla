"""Served Discover neighbour expansion (MEDULLA-FEDERATED-QUERY-R090.8).

A served `Discover` with `hops` > 0 walks the graph from the fused hits and
returns the neighbours it reaches, each with the seed-to-hit path.
"""

from __future__ import annotations

import hashlib
import os
import uuid
from typing import Any

import msgpack
import pytest
from conftest import request_context
from expansion_harness import ChatStub, connect, served_engine


def _client() -> Any:
    from medulla.client import SyncMedullaClient

    return SyncMedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
    )


def _decode(value: Any) -> Any:
    if isinstance(value, (bytes, bytearray)):
        return msgpack.unpackb(value, raw=False)
    return value


def _node(client: Any, graph: str, node_id: str, props: dict[str, Any]) -> None:
    client._send(
        "AddNode",
        {"node_id": node_id, "properties_msgpack": msgpack.packb(props)},
        graph,
    )


def _edge(client: Any, graph: str, src: str, tgt: str, weight: float) -> None:
    client._send(
        "AddEdge",
        {
            "source_id": src,
            "target_id": tgt,
            "properties_msgpack": msgpack.packb({"weight": weight}),
        },
        graph,
    )


def _discover(client: Any, graph: str, hops: int) -> list[dict[str, Any]]:
    envelope = _decode(
        client._send(
            "Discover",
            {"keywords": ["lighthouse"], "query_embedding": [], "k": 10, "hops": hops},
            graph,
        )
    )
    assert {"hits", "verdict", "plan"} <= set(envelope)
    return envelope["hits"]


@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R090.8")
def test_two_hop_neighbour_is_returned_with_path(start_medulla_server) -> None:
    """Verifies MEDULLA-FEDERATED-QUERY-R090.8."""
    graph = f"hops-{uuid.uuid4().hex[:8]}"
    client = _client()
    client.tenants.create(graph)
    try:
        _node(client, graph, "seed", {"type": "doc", "name": "lighthouse keeper"})
        _node(client, graph, "mid", {"type": "doc", "name": "harbour"})
        _node(client, graph, "far", {"type": "doc", "name": "ship"})
        _edge(client, graph, "seed", "mid", 0.9)
        _edge(client, graph, "mid", "far", 0.8)

        flat = _discover(client, graph, 0)
        assert [hit["id"] for hit in flat] == ["seed"]

        one = {hit["id"]: hit for hit in _discover(client, graph, 1)}
        assert set(one) == {"seed", "mid"}

        two = {hit["id"]: hit for hit in _discover(client, graph, 2)}
        assert set(two) == {"seed", "mid", "far"}
        assert two["far"]["hop_path"] == ["seed", "mid", "far"]
        assert two["mid"]["hop_path"] == ["seed", "mid"]
        assert "hop_path" not in two["seed"]
        assert two["far"]["score"] == pytest.approx(two["seed"]["score"] * 0.25)
    finally:
        client.tenants.delete(graph)


ANSWER = [
    "a short passage about lighthouse keepers",
    "lighthouse history",
    "keeper duties",
]


def _deep(client: Any, graph: str, **extra: Any) -> dict[str, Any]:
    args = {
        "keywords": ["lighthouse"],
        "query_embedding": [],
        "k": 10,
        "mode": "deep",
        "query_text": "who keeps the lighthouse",
        **extra,
    }
    return _decode(client._send("Discover", args, graph))


def _seed_dated(client: Any, graph: str) -> None:
    now = 1_700_000_000
    day = 86_400
    _node(
        client,
        graph,
        "fresh",
        {"type": "doc", "name": "lighthouse log", "updated_at": now},
    )
    _node(
        client,
        graph,
        "old",
        {"type": "doc", "name": "lighthouse log", "updated_at": now - 400 * day},
    )
    _node(client, graph, "undated", {"type": "doc", "name": "lighthouse log"})
    _node(client, graph, "next", {"type": "doc", "name": "harbour"})
    _edge(client, graph, "fresh", "next", 0.9)


@pytest.fixture(scope="module")
def stub_engine(tmp_path_factory):
    stub = ChatStub(ANSWER)
    with served_engine(tmp_path_factory, stub) as sock:
        yield sock, stub
    stub.close()


@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R090.2")
def test_deep_recency_multihop(stub_engine) -> None:
    """Verifies MEDULLA-FEDERATED-QUERY-R090.2."""
    sock, _ = stub_engine
    client = connect(sock)
    graph = f"deep-{uuid.uuid4().hex[:8]}"
    client.tenants.create(graph)
    try:
        _seed_dated(client, graph)
        out = _deep(client, graph, as_of=1_700_000_000, hops=1)
        hits = {hit["id"]: hit for hit in out["hits"]}
        assert hits["fresh"]["recency_boost"] == pytest.approx(1.5)
        assert hits["old"]["recency_boost"] < hits["fresh"]["recency_boost"]
        assert hits["undated"]["recency_boost"] == 1.0
        order = [hit["id"] for hit in out["hits"] if hit["id"] != "next"]
        assert order[0] == "fresh"
        assert 1.0 < hits["old"]["recency_boost"] < hits["fresh"]["recency_boost"]
        assert hits["next"]["hop_path"] == ["fresh", "next"]
        assert any(r["arm"] == "keyword" for r in hits["fresh"]["arm_ranks"])
    finally:
        client.tenants.delete(graph)


@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R090.3")
def test_deep_mode_calls_one_provider_per_request(stub_engine) -> None:
    """Verifies MEDULLA-FEDERATED-QUERY-R090.3."""
    sock, stub = stub_engine
    client = connect(sock)
    graph = f"one-{uuid.uuid4().hex[:8]}"
    client.tenants.create(graph)
    try:
        _seed_dated(client, graph)
        before = stub.calls
        _deep(client, graph)
        assert stub.calls == before + 1
        _decode(
            client._send(
                "Discover",
                {"keywords": ["lighthouse"], "query_embedding": [], "k": 5},
                graph,
            )
        )
        assert stub.calls == before + 1, "standard mode never asks the provider"
    finally:
        client.tenants.delete(graph)


@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R090.5")
def test_discover_evidence_fields(stub_engine) -> None:
    """Verifies MEDULLA-FEDERATED-QUERY-R090.5."""
    sock, _ = stub_engine
    client = connect(sock)
    graph = f"evi-{uuid.uuid4().hex[:8]}"
    client.tenants.create(graph)
    try:
        _seed_dated(client, graph)
        out = _deep(client, graph)
        expansions = out["plan"]["expansions"]
        assert [e["kind"] for e in expansions] == [
            "hypothetical",
            "sub_query",
            "sub_query",
        ]
        for evidence, text in zip(expansions, ANSWER, strict=True):
            assert evidence["digest"] == hashlib.sha256(text.encode()).hexdigest()
            assert evidence["provider_id"] == "http-chat:stub"
            assert len(evidence["provider_revision"]) == 64
        for hit in out["hits"]:
            assert "recency_boost" in hit
            assert hit["arm_ranks"], "every seed carries the arms that returned it"
    finally:
        client.tenants.delete(graph)


@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R090.4")
def test_discover_refusals(tmp_path_factory) -> None:
    """Verifies MEDULLA-FEDERATED-QUERY-R090.4."""
    with served_engine(tmp_path_factory, None) as sock:
        client = connect(sock)
        graph = f"ref-{uuid.uuid4().hex[:8]}"
        client.tenants.create(graph)
        try:
            _seed_dated(client, graph)
            with pytest.raises(Exception, match="DISCOVER_PLANNER_UNCONFIGURED"):
                _deep(client, graph)
            with pytest.raises(Exception, match="INVALID_ARGUMENT"):
                _deep(client, graph, mode="standard", hops=3)
            with pytest.raises(Exception, match="QUERY_TEXT_TOO_LONG"):
                _deep(client, graph, query_text="x" * 2049)
        finally:
            client.tenants.delete(graph)
    failing = ChatStub(None, status=500)
    try:
        with served_engine(tmp_path_factory, failing) as sock:
            client = connect(sock)
            graph = f"deg-{uuid.uuid4().hex[:8]}"
            client.tenants.create(graph)
            try:
                _seed_dated(client, graph)
                out = _deep(client, graph)
                assert out["plan"]["degraded"]["state"] == "provider_failed"
                assert out["plan"]["expansions"] == []
                assert {hit["id"] for hit in out["hits"]} >= {"fresh", "old"}
            finally:
                client.tenants.delete(graph)
    finally:
        failing.close()
