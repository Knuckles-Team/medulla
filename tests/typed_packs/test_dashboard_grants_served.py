"""Served dashboard revision-checked delete and single-grant ops.

Drives `DashboardAsset` through the real served surface as three registered
agents: the owner, a member of a granted group and a stranger.
"""

from __future__ import annotations

import os
import uuid
from collections.abc import Iterator
from typing import Any

import pytest
import stewardship_support as support
from conftest import request_context

from medulla.client import SyncMedullaClient

OWNER = "agent-dashboard-owner"
MEMBER = "agent-dashboard-member"
STRANGER = "agent-dashboard-stranger"
GROUP = "group:sales"
DASH = "orders-grants"


def _connect(graph: str | None, agent_id: str, roles: list[str] | None = None) -> Any:
    return SyncMedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        graph_name=graph,
        verified_context=request_context(agent_id=agent_id, roles=roles),
    )


def _dash(client: Any, graph: str, op: dict[str, Any]) -> dict[str, Any]:
    return dict(support.decode(client._send("DashboardAsset", {"op": op}, graph)))


def _save(client: Any, graph: str, revision: int) -> dict[str, Any]:
    body = {
        "panels": [{"id": "p0", "source": "orders", "query": "SELECT 1"}],
        "layout": {},
        "parameters": {},
    }
    op = {"op": "save", "id": DASH, "body": body, "revision": revision}
    return _dash(client, graph, op)


def _node(client: Any, graph: str, node_id: str) -> dict[str, Any]:
    raw = client._send("GetNodeProperties", {"node_id": node_id}, graph)
    return dict(support.decode(raw) or {})


@pytest.fixture
def served() -> Iterator[tuple[str, Any, Any, Any]]:
    graph = f"medulla-r109-grants-{uuid.uuid4().hex}"
    admin = _connect(None, "test-system")
    created = False
    try:
        admin.tenants.create(graph, graph_type="Global")
        created = True
        support.register_agents(graph, OWNER, MEMBER, STRANGER)
        owner = _connect(graph, OWNER)
        member = _connect(graph, MEMBER, roles=[GROUP])
        stranger = _connect(graph, STRANGER)
        try:
            yield graph, owner, member, stranger
        finally:
            for client in (owner, member, stranger):
                client.close()
    finally:
        try:
            if created:
                admin.tenants.delete(graph)
        finally:
            admin.close()


@pytest.mark.spec("MEDULLA-TYPED-PACKS-R109.7")
def test_delete_with_expected_revision_refuses_a_mismatch_and_is_not_a_view(
    served: tuple[str, Any, Any, Any],
) -> None:
    # spec: MEDULLA-TYPED-PACKS-R109.7
    graph, owner, member, _ = served
    _save(owner, graph, 0)
    _save(owner, graph, 1)
    grant = {"principal_or_group": GROUP, "role": "view"}
    _dash(owner, graph, {"op": "share", "id": DASH, "grants": [grant]})
    _dash(member, graph, {"op": "get", "id": DASH})
    before = _node(owner, graph, f"dashboard:{DASH}")
    assert before["usage_count"] == 1

    # A stale expected revision refuses and writes nothing.
    stale = {"op": "delete", "id": DASH, "expected_revision": 1}
    with pytest.raises(Exception, match="DASHBOARD_REVISION_CONFLICT"):
        _dash(owner, graph, stale)
    assert _node(owner, graph, f"dashboard:{DASH}")["removed_at_ms"] is None

    # The current revision deletes, and the delete is not counted as a view.
    current = {"op": "delete", "id": DASH, "expected_revision": 2}
    deleted = _dash(owner, graph, current)
    assert deleted["removed_at_ms"] > 0
    assert deleted["usage_count"] == before["usage_count"]
    assert deleted["last_viewed_ms"] == before["last_viewed_ms"]
    after = _node(owner, graph, f"dashboard:{DASH}")
    assert after["usage_count"] == before["usage_count"]
    assert after["last_viewed_ms"] == before["last_viewed_ms"]
