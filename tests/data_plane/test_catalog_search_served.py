"""Served asset usage (MEDULLA-UNIFIED-DATA-PLANE-R764.3.1).

The server records which datasets a served query referenced, per verified
principal, in the `asset_usage.reads` and `asset_usage.queries` series. The count
comes only from the server's own dispatch: a usage claim sent by the client is
not a field of any method and changes nothing.
"""

from __future__ import annotations

import os
import uuid
from typing import Any

import msgpack
import pytest
from conftest import TEST_AGENT_ID, request_context


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


def _sql(client: Any, graph: str, query: str) -> Any:
    return _decode(client._send("Sql", {"query": query, "params_msgpack": b""}, graph))


def _usage_points(client: Any, graph: str, base: str, asset: str) -> int:
    series = f"{base}|principal={TEST_AGENT_ID}|asset={asset}"
    rows = _decode(
        client._send("TsRange", {"series_id": series, "from": 0, "to": 2**62}, graph)
    )
    return sum(int(sum(values)) for _ts, values in rows)


@pytest.mark.spec("MEDULLA-UNIFIED-DATA-PLANE-R764.3.1")
def test_usage_counts_come_from_served_queries(start_medulla_server) -> None:
    """Verifies MEDULLA-UNIFIED-DATA-PLANE-R764.3.1: two served reads count two."""
    graph = f"usage-{uuid.uuid4().hex[:8]}"
    client = _client()
    client.tenants.create(graph)
    try:
        assert _usage_points(client, graph, "asset_usage.reads", "nodes") == 0
        _sql(client, graph, "SELECT count(*) AS n FROM nodes")
        _sql(client, graph, "SELECT count(*) AS n FROM nodes")
        assert _usage_points(client, graph, "asset_usage.reads", "nodes") == 2
        assert _usage_points(client, graph, "asset_usage.queries", "nodes") == 2

        # A failed statement records nothing.
        with pytest.raises(Exception, match="."):
            _sql(client, graph, "SELECT nope FROM nodes WHERE")
        assert _usage_points(client, graph, "asset_usage.reads", "nodes") == 2

        # A client-supplied usage claim is not a field of any method. Whether the
        # server drops the extra field or refuses the request, it counts at most the
        # one read it served itself, never the claimed 1000.
        try:
            client._send(
                "Sql",
                {
                    "query": "SELECT count(*) AS n FROM nodes",
                    "params_msgpack": b"",
                    "asset_usage": {"asset": "nodes", "reads": 1000},
                },
                graph,
            )
        except Exception:
            pass
        assert _usage_points(client, graph, "asset_usage.reads", "nodes") in (2, 3)
    finally:
        client.tenants.delete(graph)
        client.close()
