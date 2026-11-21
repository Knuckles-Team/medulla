"""The UQL EDGES source over the served wire (MEDULLA-FEDERATED-QUERY-R735).

The browser relationship and graph views read edges as rows: one served ``Uql`` statement
returns one row per edge the caller may read, with its ``source``, ``target`` and ``rel``
and its scalar properties as typed columns. The source reads the caller's own snapshot, so a
reader whose authority hides a node never sees an edge that touches it.
"""

from __future__ import annotations

import os
import uuid

import pytest

from medulla.client import SyncMedullaClient

try:
    from conftest import TEST_TENANT, request_context
except ModuleNotFoundError:  # package-root import
    from tests.conftest import TEST_TENANT, request_context

OWNER = "usr:a"
READER = "usr:b"


def _connect(principal: str) -> SyncMedullaClient:
    context = request_context(principal=principal)
    context["tenant"] = TEST_TENANT
    return SyncMedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=context,
    )


def _edge_ids(client: SyncMedullaClient, tag: str) -> list[str]:
    result = client.query.uql(f"EDGES :dependsOn{tag} |> ORDER BY id |> LIMIT 100000")
    return [row["id"] for row in result["rows"]]


@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R735")
def test_edge_rows_respect_edge_read_scope(clean_graph) -> None:
    tag = uuid.uuid4().hex[:8]
    a, b, c = f"edge-a-{tag}", f"edge-b-{tag}", f"edge-c-{tag}"
    clean_graph.nodes.add(a, {"type": "Doc"})
    clean_graph.nodes.add(b, {"type": "Doc"})
    # `c` is a private knowledge unit of OWNER: no other principal can read it.
    clean_graph.nodes.add(
        c,
        {
            "node_type": "KnowledgeUnit",
            "owner": OWNER,
            "owner_tenant": TEST_TENANT,
            "version": 1,
            "share_scope": "private",
        },
    )
    rel = f"dependsOn{tag}"
    clean_graph.edges.add(a, b, {"relationship": rel, "weight": 0.7})
    clean_graph.edges.add(a, c, {"relationship": rel, "weight": 0.2})
    clean_graph.edges.add(b, c, {"relationship": rel})

    full = _connect(OWNER)
    restricted = _connect(READER)
    try:
        # Full read: all three edges, in key order, with typed columns.
        assert _edge_ids(full, tag) == [f"{a}|{b}|0", f"{a}|{c}|0", f"{b}|{c}|0"]
        result = full.query.uql(
            f"EDGES :{rel} WHERE weight >= 0.5 |> LIMIT 100000",
        )
        assert [row["id"] for row in result["rows"]] == [f"{a}|{b}|0"]
        assert "weight" in result["columns"]

        # A reader whose authority hides `c` gets only the edge between visible nodes.
        assert _edge_ids(restricted, tag) == [f"{a}|{b}|0"]
    finally:
        full.close()
        restricted.close()
