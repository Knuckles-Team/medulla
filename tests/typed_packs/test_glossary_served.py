"""Served business glossary (MEDULLA-TYPED-PACKS-R801).

Drives `Glossary` through the real served surface: a scheme is imported with its
terms, a term moves through review, and an approved term binds to an asset.
"""

from __future__ import annotations

import os
import uuid
from typing import Any

import msgpack
import pytest
from conftest import request_context

from medulla.client import SyncMedullaClient

SCHEME = "urn:scheme:finance"
REVENUE = "urn:term:revenue"
ASSET = "col:orders.total"


def _connect(graph: str | None = None) -> Any:
    return SyncMedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        graph_name=graph,
        verified_context=request_context(),
    )


def _decode(value: Any) -> Any:
    if isinstance(value, (bytes, bytearray)):
        return msgpack.unpackb(value, raw=False)
    return value


def _op(client: Any, graph: str, op: dict[str, Any]) -> dict[str, Any]:
    return _decode(client._send("Glossary", {"op": op}, graph))


def _props(client: Any, graph: str, node_id: str) -> dict[str, Any]:
    return _decode(client._send("GetNodeProperties", {"node_id": node_id}, graph))


def _term(status: str = "draft") -> dict[str, Any]:
    return {
        "scheme": SCHEME,
        "pref_label": "Revenue",
        "alt_labels": ["Turnover"],
        "definition": "Income from sales.",
        "status": status,
    }


@pytest.fixture
def served():
    graph = f"medulla-r801-{uuid.uuid4().hex}"
    client = _connect(None)
    created = False
    try:
        client.tenants.create(graph, graph_type="Global")
        created = True
        client._send(
            "AddNode",
            {
                "node_id": ASSET,
                "properties_msgpack": msgpack.packb({"type": "Column"}, use_bin_type=True),
            },
            graph,
        )
        yield graph, client
    finally:
        try:
            if created:
                client.tenants.delete(graph)
        finally:
            client.close()


@pytest.mark.spec("MEDULLA-TYPED-PACKS-R801")
@pytest.mark.spec("MEDULLA-TYPED-PACKS-R801.3")
def test_import_scheme_and_bind_term(served) -> None:
    """Verifies MEDULLA-TYPED-PACKS-R801.3: import a scheme, review a term, bind it."""
    graph, client = served
    put = _op(
        client,
        graph,
        {
            "op": "put_scheme",
            "scheme": SCHEME,
            "terms": [{"iri": REVENUE, "term": _term()}],
        },
    )
    assert put["iri"] == SCHEME and put["written"] == 2
    assert _props(client, graph, REVENUE)["status"] == "draft"

    # A put never moves a status: a new term must start in draft.
    with pytest.raises(Exception, match="draft"):
        _op(
            client,
            graph,
            {"op": "put_term", "iri": "urn:term:cost", "term": _term("approved")},
        )

    for event, status in (("submit", "in_review"), ("approve", "approved")):
        moved = _op(
            client,
            graph,
            {"op": "transition", "iri": REVENUE, "event": event},
        )
        assert moved["status"] == status
    assert _props(client, graph, REVENUE)["status"] == "approved"

    bound = _op(client, graph, {"op": "bind", "term": REVENUE, "asset": ASSET})
    assert bound["iri"] == REVENUE
