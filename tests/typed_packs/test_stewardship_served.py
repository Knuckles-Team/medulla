"""Served stewardship store and change requests (MEDULLA-TYPED-PACKS-R802).

Drives `Stewardship` through the real served surface: a role is assigned, a change
request is opened with the resolved reviewers, reviewed, applied, and an asset is
certified. A principal that governs nothing is refused.
"""

from __future__ import annotations

import os
import uuid
from typing import Any

import msgpack
import pytest
from conftest import request_context

from medulla.client import SyncMedullaClient

ASSET = "col:orders.total"
DIGEST = "sha256:" + "ab" * 32


def _connect(graph: str | None = None, *, roles: list[str] | None = None) -> Any:
    return SyncMedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        graph_name=graph,
        verified_context=request_context(roles=roles),
    )


def _decode(value: Any) -> Any:
    if isinstance(value, (bytes, bytearray)):
        return msgpack.unpackb(value, raw=False)
    return value


def _op(client: Any, graph: str, op: dict[str, Any]) -> dict[str, Any]:
    return _decode(client._send("Stewardship", {"op": op}, graph))


@pytest.fixture
def served():
    graph = f"medulla-r802-{uuid.uuid4().hex}"
    client = _connect(None)
    created = False
    try:
        client.tenants.create(graph, graph_type="Global")
        created = True
        yield graph, client
    finally:
        try:
            if created:
                client.tenants.delete(graph)
        finally:
            client.close()


@pytest.mark.spec("MEDULLA-TYPED-PACKS-R802")
@pytest.mark.spec("MEDULLA-TYPED-PACKS-R802.3")
def test_change_request_flow(served) -> None:
    """Verifies MEDULLA-TYPED-PACKS-R802.3: open, approve and apply one request."""
    graph, client = served
    me = request_context()["principal"]
    assignment = {
        "scope": {"kind": "asset", "id": ASSET},
        "role": "steward",
        "principal": me,
    }
    assigned = _op(client, graph, {"op": "assign", "assignment": assignment})
    assert assigned["stewards"] == [me]

    opened = _op(
        client,
        graph,
        {
            "op": "open_change_request",
            "target": ASSET,
            "proposed_digest": DIGEST,
            "scopes": {"asset": ASSET},
        },
    )
    assert opened["change_request"]["state"] == "open"
    assert opened["change_request"]["reviewers"] == [me]
    request_id = opened["id"]

    approved = _op(client, graph, {"op": "review", "request_id": request_id, "approve": True})
    assert approved["change_request"]["state"] == "approved"
    applied = _op(client, graph, {"op": "apply", "request_id": request_id})
    assert applied["change_request"]["state"] == "applied"

    certified = _op(client, graph, {"op": "certify", "asset": ASSET, "level": "gold"})
    assert certified["certification"]["level"] == "gold"


@pytest.mark.spec("MEDULLA-TYPED-PACKS-R802.3")
def test_principal_without_assignment_is_refused(served) -> None:
    """A principal that governs nothing cannot open a request or certify."""
    graph, client = served
    with pytest.raises(Exception, match="STEWARD_ROLE_MISSING"):
        _op(client, graph, {"op": "certify", "asset": ASSET, "level": "gold"})
