"""The hand-written client reads the graph change feed from a served engine
(MEDULLA-DURABLE-KERNEL-R043.100.4).

Both methods are driven over the framed-MessagePack socket the conftest fixture
serves: a snapshot, an ordered page of committed changes after its cursor, a
resume from the page's cursor, and the typed refusal of a cursor that a graph
clear made stale, which names a safe cursor that reads again.
"""

from __future__ import annotations

import os

import pytest
from conftest import request_context

from medulla.client import EngineResponseError, SyncMedullaClient


@pytest.fixture
def client():
    socket_path = os.environ["GRAPH_SERVICE_SOCKET"]
    c = SyncMedullaClient.connect(
        socket_path=socket_path, verified_context=request_context()
    )
    c.graph.clear()
    return c


def _upserted(page):
    ids = []
    for event in page["events"]:
        assert event["change"]["kind"] == "upsert"
        ids.extend(event["change"]["node_ids"])
    return ids


def test_client_reads_feed_from_a_served_engine(client):
    client.nodes.add("before-snapshot", {"type": "Doc"})
    snapshot = client.graph.open_graph_snapshot()
    assert snapshot["schema_version"] == 1
    assert len(snapshot["snapshot_digest"]) == 64
    assert snapshot["high_water_cursor"]

    # Commits after the snapshot arrive in order from its cursor, and only they.
    client.nodes.add("n1", {"type": "Doc"})
    client.nodes.add("n2", {"type": "Doc"})
    page = client.graph.read_graph_changes(snapshot["high_water_cursor"], limit=10)
    assert page["schema_version"] == 1
    assert _upserted(page) == ["n1", "n2"]
    ordinals = [event["event_ordinal"] for event in page["events"]]
    assert ordinals == sorted(ordinals)
    keys = {(event["commit_id"], event["event_ordinal"]) for event in page["events"]}
    assert len(keys) == len(page["events"]), "each event has its own stable id"
    for event in page["events"]:
        assert event["commit_to_visible_ms"] < 1_000

    # Resuming from the page's cursor returns nothing seen, then only the new commit.
    assert (
        client.graph.read_graph_changes(page["next_cursor"], limit=10)["events"] == []
    )
    client.nodes.add("n3", {"type": "Doc"})
    tail = client.graph.read_graph_changes(page["next_cursor"], limit=10)
    assert _upserted(tail) == ["n3"]

    # A page size outside 1..=256 is refused, not clamped.
    for bad in (0, 257):
        with pytest.raises(EngineResponseError) as refused:
            client.graph.read_graph_changes(page["next_cursor"], limit=bad)
        assert refused.value.code == "INVALID_ARGUMENT"

    # A graph clear replaces the feed: the old cursor is stale and names a safe one.
    client.graph.clear()
    with pytest.raises(EngineResponseError) as stale:
        client.graph.read_graph_changes(snapshot["high_water_cursor"], limit=10)
    assert stale.value.code == "GRAPH_INCARNATION_STALE"
    safe = (stale.value.detail or "").split("safe_cursor=")[1].strip()
    client.nodes.add("after-clear", {"type": "Doc"})
    assert _upserted(client.graph.read_graph_changes(safe, limit=10)) == ["after-clear"]
