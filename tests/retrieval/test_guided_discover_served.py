"""Guided `Discover`: caller seeds and a per-hop filter, served
(MEDULLA-FEDERATED-QUERY-R741, R741.4).

One graph holds two seeds and three neighbours: `n-cites` and `n-hidden` are cited by
a seed, `n-knows` is known by a seed, and `n-hidden` is private to another owner. The
system identity reads every row; a registered reader sees every row but `n-hidden`.
"""

from __future__ import annotations

import os
import uuid
from typing import Any

import msgpack
import pytest
from _round_served import client, put
from conftest import register_commons_agents, request_context

from medulla.client import SyncMedullaClient

READER = "agent:guided-reader"
OWNER = "service:guided-owner"


def _public(name: str) -> dict[str, Any]:
    return {
        "type": "Doc",
        "name": name,
        "_owner": OWNER,
        "_visibility": "public",
    }


def _edge(cl: Any, graph: str, source: str, target: str, label: str) -> None:
    cl._send(
        "AddEdge",
        {
            "source_id": source,
            "target_id": target,
            "properties_msgpack": msgpack.packb({"relationship": label}),
        },
        graph,
    )


def _discover(cl: Any, graph: str, **params: Any) -> dict[str, Any]:
    body: dict[str, Any] = {"keywords": [], "query_embedding": [], "k": 32, **params}
    result = cl._send("Discover", body, graph)
    if isinstance(result, (bytes, bytearray)):
        result = msgpack.unpackb(result, raw=False)
    return result


def _ids(cl: Any, graph: str, **params: Any) -> list[str]:
    return sorted(hit["id"] for hit in _discover(cl, graph, **params)["hits"])


def _seed_graph(cl: Any, graph: str) -> None:
    for node_id in ("seed-1", "seed-2", "n-cites", "n-knows"):
        put(cl, graph, node_id, _public(node_id))
    put(
        cl,
        graph,
        "n-hidden",
        {"type": "Doc", "name": "n-hidden", "_owner": OWNER, "_visibility": "private"},
    )
    _edge(cl, graph, "seed-1", "n-cites", "CITES")
    _edge(cl, graph, "seed-1", "n-knows", "KNOWS")
    _edge(cl, graph, "seed-2", "n-hidden", "CITES")


@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R741")
@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R741.4")
def test_seeded_walk_respects_hop_filter_and_rls(start_medulla_server) -> None:
    """Verifies MEDULLA-FEDERATED-QUERY-R741.4: a walk from two seeds over one edge
    label returns the filtered neighbours to the system caller, and never returns the
    hidden node to the reader; a seed the reader cannot read refuses like an absent
    one."""
    graph = f"guided-discover-{uuid.uuid4().hex[:8]}"
    system = client()
    system.tenants.create(graph)
    reader = None
    try:
        _seed_graph(system, graph)
        register_commons_agents(READER, graph=graph)
        reader = SyncMedullaClient.connect(
            socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
            verified_context=request_context(agent_id=READER),
        )
        walk = {
            "seed_ids": ["seed-1", "seed-2"],
            "hops": 1,
            "hop_filter": {"edge_labels": ["CITES"]},
        }
        assert _ids(system, graph, **walk) == [
            "n-cites",
            "n-hidden",
            "seed-1",
            "seed-2",
        ]
        assert _ids(reader, graph, **walk) == ["n-cites", "seed-1", "seed-2"]

        # No filter restricts nothing: the same seeds reach every readable neighbour.
        open_walk = {"seed_ids": ["seed-1", "seed-2"], "hops": 1}
        assert _ids(reader, graph, **open_walk) == [
            "n-cites",
            "n-knows",
            "seed-1",
            "seed-2",
        ]

        # Seeds come back in the given order, each scoring one, with no arm run.
        hits = _discover(system, graph, seed_ids=["seed-2", "seed-1"])["hits"]
        assert [hit["id"] for hit in hits] == ["seed-2", "seed-1"]
        assert all(hit["score"] == 1.0 for hit in hits)

        # A hidden seed and an absent seed refuse with the same text.
        with pytest.raises(Exception, match="SEED_NOT_VISIBLE") as hidden:
            _discover(reader, graph, seed_ids=["n-hidden"])
        with pytest.raises(Exception, match="SEED_NOT_VISIBLE") as absent:
            _discover(reader, graph, seed_ids=["n-absent"])
        assert str(hidden.value) == str(absent.value)

        # The count and the filter refuse before any retrieval.
        with pytest.raises(Exception, match="SEED_LIMIT_EXCEEDED"):
            _discover(system, graph, seed_ids=[f"s{i}" for i in range(65)])
        with pytest.raises(Exception, match="HOP_FILTER_INVALID"):
            _discover(
                system,
                graph,
                seed_ids=["seed-1"],
                hop_filter={"language": "not a tag"},
            )
    finally:
        if reader is not None:
            reader.close()
        system.tenants.delete(graph)
        system.close()
