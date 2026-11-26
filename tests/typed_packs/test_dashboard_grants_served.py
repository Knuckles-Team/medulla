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


def _grants(receipt: dict[str, Any]) -> dict[str, str]:
    return {g["principal_or_group"]: g["role"] for g in receipt["grants"]}


@pytest.mark.spec("MEDULLA-TYPED-PACKS-R109.8")
def test_grant_and_revoke_change_one_grantee_without_a_whole_list_share(
    served: tuple[str, Any, Any, Any],
) -> None:
    # spec: MEDULLA-TYPED-PACKS-R109.8
    graph, owner, member, stranger = served
    _save(owner, graph, 0)

    def grant(principal: str, role: str) -> dict[str, Any]:
        grant = {"principal_or_group": principal, "role": role}
        return _dash(owner, graph, {"op": "grant", "id": DASH, "grant": grant})

    # Two grants for different grantees both stay; neither carries the other's list.
    assert _grants(grant(GROUP, "view")) == {GROUP: "view"}
    assert _grants(grant("group:ops", "edit")) == {GROUP: "view", "group:ops": "edit"}
    # Granting again changes only that grantee's role.
    both = _grants(grant(GROUP, "edit"))
    assert both == {GROUP: "edit", "group:ops": "edit"}
    stored = _node(owner, graph, f"dashboard:{DASH}")
    assert stored["grants_by"] == support.principal(OWNER)
    assert stored["grants_at_ms"] > 0

    # The member holds the group grant now, and a stranger still holds none.
    assert _dash(member, graph, {"op": "get", "id": DASH})["id"] == DASH
    with pytest.raises(Exception, match="DASHBOARD_NOT_GRANTED"):
        _dash(stranger, graph, {"op": "get", "id": DASH})

    # A revoke removes one grantee; revoking a grantee with no grant succeeds.
    revoke = {"op": "revoke", "id": DASH, "principal_or_group": "group:ops"}
    assert _grants(_dash(owner, graph, revoke)) == {GROUP: "edit"}
    assert _grants(_dash(owner, graph, revoke)) == {GROUP: "edit"}

    # Only the owner changes a grant.
    grant_op = {"principal_or_group": GROUP, "role": "view"}
    with pytest.raises(Exception, match="DASHBOARD_NOT_GRANTED"):
        _dash(member, graph, {"op": "grant", "id": DASH, "grant": grant_op})
    with pytest.raises(Exception, match="DASHBOARD_NOT_GRANTED"):
        _dash(member, graph, {**revoke, "principal_or_group": GROUP})
