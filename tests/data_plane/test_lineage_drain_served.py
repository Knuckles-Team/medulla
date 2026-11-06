"""Served lineage drain (MEDULLA-UNIFIED-DATA-PLANE-R702.3 / R702.6).

`LineageIngest` maps run events to content-addressed `Dataset` nodes and
`derivesInto` edges in the request graph. A replayed event adds no edge and
an event that cannot map is kept as one `QuarantinedLineageEvent` node.
"""

from __future__ import annotations

import hashlib
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


def _dataset_id(namespace: str, name: str) -> str:
    return "ds:" + hashlib.sha256(f"{namespace}/{name}".encode()).hexdigest()


def _event(
    event_id: str, inputs: list[tuple[str, str]], outputs: list[tuple[str, str]]
) -> dict:
    return {
        "event_id": event_id,
        "run": {
            "run_id": event_id,
            "job": "materialize.engine.t",
            "started_at_ms": 1,
            "ended_at_ms": 2,
        },
        "inputs": [{"namespace": ns, "name": name} for ns, name in inputs],
        "outputs": [{"namespace": ns, "name": name} for ns, name in outputs],
    }


def _ingest(client: Any, graph: str, events: list[dict]) -> dict:
    return _decode(client._send("LineageIngest", {"events": events}, graph))


def _has_edge(client: Any, graph: str, source: str, target: str) -> bool:
    return bool(
        _decode(
            client._send("HasEdge", {"source_id": source, "target_id": target}, graph)
        )
    )


@pytest.mark.spec("MEDULLA-UNIFIED-DATA-PLANE-R702.3")
def test_lake_sweep_run_event_reaches_lineage_graph(start_medulla_server) -> None:
    """Verifies MEDULLA-UNIFIED-DATA-PLANE-R702.3: sweep-fed lineage wiring."""
    graph = f"lineage-{uuid.uuid4().hex[:8]}"
    client = _client()
    try:
        # The shape the lake sweep submits: a tsdb series drained into a table.
        event = _event(
            "run-sweep-1", [("medulla.tsdb", "s1")], [("engine", "engine.s1")]
        )
        receipt = _ingest(client, graph, [event])
        assert receipt["edges"] == 1
        assert _has_edge(
            client,
            graph,
            _dataset_id("medulla.tsdb", "s1"),
            _dataset_id("engine", "engine.s1"),
        )
    finally:
        client.close()


@pytest.mark.spec("MEDULLA-UNIFIED-DATA-PLANE-R702.6")
def test_replay_adds_no_edges_and_bad_events_quarantine(start_medulla_server) -> None:
    """Verifies MEDULLA-UNIFIED-DATA-PLANE-R702.6: replay and quarantine proof."""
    graph = f"lineage-{uuid.uuid4().hex[:8]}"
    client = _client()
    try:
        valid = [
            _event("e1", [("ns", "a")], [("ns", "b")]),
            _event("e2", [("ns", "b")], [("ns", "c")]),
            _event("e3", [("ns", "c")], [("ns", "d")]),
        ]
        first = _ingest(client, graph, valid)
        assert first["edges"] == 3
        edges_after_first = _decode(client._send("EdgeCount", {}, graph))
        replay = _ingest(client, graph, valid)
        assert replay["edges"] == 0
        assert _decode(client._send("EdgeCount", {}, graph)) == edges_after_first
        malformed = [
            _event("bad1", [("ns", "has space")], [("ns", "b")]),
            _event("bad2", [], [("ns", "b")]),
        ]
        bad = _ingest(client, graph, malformed)
        assert bad["quarantined"] == 2
        nodes = _decode(
            client._send(
                "GetNodesByLabel",
                {"label": "QuarantinedLineageEvent", "limit": 0},
                graph,
            )
        )
        assert len(nodes) == 2
        assert _decode(client._send("EdgeCount", {}, graph)) == edges_after_first
    finally:
        client.close()
