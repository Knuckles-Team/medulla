"""Served viewport bins through the client (MEDULLA-FEDERATED-QUERY-R081.103.4).

`ViewportQuery` answers a bounded bin set whatever the size of the series;
`ResolveViewportSelection` turns a bin token back into raw points inside its pinned
revision.
"""

from __future__ import annotations

import os
import uuid
from typing import Any

import msgpack
import pytest
from conftest import request_context

_POINTS = 20_000
_STEP_NS = 1_000_000


def _client() -> Any:
    from medulla.client import SyncMedullaClient

    return SyncMedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
    )


def _unpack(result: Any) -> dict[str, Any]:
    if isinstance(result, (bytes, bytearray)):
        result = msgpack.unpackb(result, raw=False)
    assert isinstance(result, dict)
    return result


def _seed(client: Any, graph: str) -> None:
    points = [[i * _STEP_NS, [float((i * 31) % 977)]] for i in range(_POINTS)]
    client._send(
        "TsAppend",
        {
            "series_id": "load",
            "n_fields": 1,
            "bucket_ns": 10_000_000_000,
            "field_names": ["v"],
            "points_msgpack": msgpack.packb(points),
        },
        graph,
    )


def _request(width: int) -> dict[str, Any]:
    return {
        "source_id": "load",
        "kind": "line",
        "x_axis": "ts",
        "y_axis": "v",
        "units": "ms",
        "domain": {
            "x_min": 0.0,
            "x_max": float((_POINTS - 1) * _STEP_NS),
            "y_min": 0.0,
            "y_max": 1000.0,
        },
        "pixel_width": width,
        "pixel_height": 100,
        "binning_revision": 1,
        "byte_ceiling": 16_777_216,
    }


@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R081.103.4")
def test_viewport_round_trip(start_medulla_server) -> None:
    """Verifies MEDULLA-FEDERATED-QUERY-R081.103.4: bounded bins and token resolve."""
    graph = f"vp-{uuid.uuid4().hex[:8]}"
    client = _client()
    client.tenants.create(graph)
    try:
        _seed(client, graph)
        for width in (50, 400):
            result = _unpack(
                client._send("ViewportQuery", {"request": _request(width)}, graph)
            )
            bins = result["bins"]["bins"]
            assert result["bins"]["kind"] == "line"
            assert len(bins) == width
            assert sum(b[5] for b in bins) == _POINTS
            assert result["source_cardinality"] == _POINTS
            assert len(result["query_digest"]) == 64

        pinned = _request(50)
        result = _unpack(client._send("ViewportQuery", {"request": pinned}, graph))
        token = {
            "kind": "aggregate_bin",
            "bin_id": result["bins"]["bins"][2][0],
            "source_revision": result["source_revision"],
            "index_revision": result["binning_revision"],
            "viewport_digest": result["query_digest"],
        }
        resolved = _unpack(
            client._send(
                "ResolveViewportSelection", {"request": pinned, "token": token}, graph
            )
        )
        assert resolved["total_count"] == result["bins"]["bins"][2][5]
        assert resolved["truncated"] is False

        # A pan changes the digest.
        panned = _request(50)
        panned["domain"]["x_min"] = float(_STEP_NS * 1000)
        other = _unpack(client._send("ViewportQuery", {"request": panned}, graph))
        assert other["query_digest"] != result["query_digest"]
    finally:
        client.tenants.delete(graph)
        client.close()
