"""Served Discover neighbour expansion (MEDULLA-FEDERATED-QUERY-R090.8).

A served `Discover` with `hops` > 0 walks the graph from the fused hits and
returns the neighbours it reaches, each with the seed-to-hit path.
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
    return _decode(
        client._send(
            "Discover",
            {"keywords": ["lighthouse"], "query_embedding": [], "k": 10, "hops": hops},
            graph,
        )
    )


@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R090.8")
def test_two_hop_neighbour_is_returned_with_path(start_medulla_server) -> None:
    """Verifies MEDULLA-FEDERATED-QUERY-R090.8: two-hop neighbour and its path."""
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
        assert two["far"]["path"] == ["seed", "mid", "far"]
        assert two["mid"]["path"] == ["seed", "mid"]
        assert "path" not in two["seed"]
        assert two["far"]["score"] == pytest.approx(two["seed"]["score"] * 0.25)
    finally:
        client.tenants.delete(graph)
