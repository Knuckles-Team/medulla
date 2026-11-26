"""Served acceptance of event-stream sources (MEDULLA-FEDERATED-QUERY-R073.2).

The matrix row of a stream protocol permits a time range and nothing else. A registered
``EventStream`` source reads through its stream tool with only the window it declares:
a limit that follows the scan, or the local ids of a join, never reach the tool. The
gateway is a loopback stub that records each call.
"""

from __future__ import annotations

import pytest

from .conftest import STREAM_EVENTS, STREAM_SERVER, STREAM_TOOL, WebGateway

STREAM_SOURCE = {
    "EventStream": {
        "server": STREAM_SERVER,
        "tool": STREAM_TOOL,
        "from_ms": 1_000,
        "to_ms": 2_000,
    }
}


def scan(name: str) -> dict[str, object]:
    return {"ForeignScan": {"source": {"Named": {"name": name}}, "join": False}}


@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R073.2")
def test_stream_pushes_time_range_only(clean_graph, web_gateway: WebGateway):
    clean_graph.query.register_foreign_source("events", STREAM_SOURCE)
    before = len(web_gateway.stream_calls)
    rows = clean_graph.query.unified([scan("events")])
    assert [row["id"] for row in rows if row["id"] in STREAM_EVENTS] == STREAM_EVENTS
    sent = web_gateway.stream_calls[before]["arguments"]
    assert sent == {"from_ms": 1_000, "to_ms": 2_000}

    # A limit after the scan is the local residual: the tool still gets the window only.
    limited = clean_graph.query.unified([scan("events"), {"Limit": {"k": 1}}])
    assert len(limited) == 1
    assert web_gateway.stream_calls[before + 1]["arguments"] == {
        "from_ms": 1_000,
        "to_ms": 2_000,
    }
