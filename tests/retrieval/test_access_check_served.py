"""Read-time access checks over served ContextCompile (MEDULLA-FEDERATED-QUERY-R703).

A source that does not sync its ACLs declares `access_mode: checked_at_read`. The
engine asks that source's connector, through the fleet gateway the federation
fixtures stand on loopback, which of the hits the caller may read, and a denied
record never enters the bundle.
"""

from __future__ import annotations

import os
import uuid
from typing import Any

import msgpack
import pytest
from conftest import request_context
from federation.conftest import WebGateway, web_gateway  # noqa: F401

SOURCE = "acl-source"
SERVER = "acl-mcp"
TEXT = "lighthouse keeper logbook entry written through the long winter nights"


def _client() -> Any:
    from medulla.client import SyncMedullaClient

    return SyncMedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
    )


def _node(client: Any, node_id: str, props: dict[str, Any]) -> None:
    client._send(
        "AddNode",
        {"node_id": node_id, "properties_msgpack": msgpack.packb(props)},
        None,
    )


def _request() -> dict[str, Any]:
    return {
        "query_text": "lighthouse keeper logbook",
        "query_embedding": [],
        "budget": {
            "window_tokens": 4000,
            "reserved_output_tokens": 0,
            "caller_ceiling_tokens": None,
            "money_ceiling_micro": None,
            "price_micro_per_token": None,
            "latency_ceiling_ms": None,
            "tokens_per_second": None,
        },
        "tokenizer_id": "bytes-estimate",
        "tokenizer_version": "1",
        "mode": "lexical",
        "top_n": 256,
    }


def _compile(client: Any) -> dict[str, Any]:
    result = client._send("ContextCompile", {"request": _request()}, None)
    if isinstance(result, (bytes, bytearray)):
        result = msgpack.unpackb(result, raw=False)
    return result


def _checked_fixture(client: Any, suffix: str) -> tuple[str, str, str]:
    allowed, denied, synced = (
        f"ac-{suffix}-{n}" for n in ("allowed", "denied", "synced")
    )
    client.query.register_foreign_source(
        SOURCE, {"Mcp": {"server": SERVER, "tool": "records"}}
    )
    for node_id in (allowed, denied):
        _node(
            client,
            node_id,
            {
                "type": "document",
                "name": f"lighthouse logbook {node_id}",
                "description": TEXT,
                "source": SOURCE,
                "access_mode": "checked_at_read",
            },
        )
    _node(
        client,
        synced,
        {
            "type": "document",
            "name": f"lighthouse logbook {synced}",
            "description": TEXT,
        },
    )
    return allowed, denied, synced


@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R703.3")
def test_denied_hit_never_enters_bundle(
    clean_graph: Any,
    web_gateway: WebGateway,  # noqa: F811
) -> None:
    suffix = uuid.uuid4().hex[:8]
    allowed, denied, synced = _checked_fixture(clean_graph, suffix)
    web_gateway.denied_refs = {denied}
    web_gateway.access_calls.clear()

    bundle = _compile(clean_graph)

    selected = {item["item_id"] for item in bundle["items"]}
    assert denied not in selected
    assert {allowed, synced} <= selected
    assert {"item_id": denied, "reason": "denied"} in bundle["excluded"]
    assert {"record_id": denied, "reason": "denied"} in bundle["evidence"][
        "unchecked_dropped"
    ]

    # One check reached the source's connector: only the checked-at-read records, the
    # synced record is never sent, and the verified caller is the principal.
    assert len(web_gateway.access_calls) == 1
    call = web_gateway.access_calls[0]
    assert call["server"] == SERVER
    assert call["tool"] == "source_access_check"
    assert sorted(call["record_refs"]) == sorted([allowed, denied])
    assert call["principal"]
    assert call["timeout_ms"] == 2000
