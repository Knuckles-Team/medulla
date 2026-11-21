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
import stewardship_support as support
from conftest import request_context
from stewardship_support import DIGEST, REVIEWER, STRANGER

from medulla.client import SyncMedullaClient

ASSET = "col:orders.total"


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
        support.register_agents(graph, REVIEWER, STRANGER)
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
    other = support.principal(REVIEWER)
    for who in (me, other):
        assigned = _op(
            client,
            graph,
            {
                "op": "assign",
                "assignment": {
                    "scope": {"kind": "asset", "id": ASSET},
                    "role": "steward",
                    "principal": who,
                },
            },
        )
    assert assigned["stewards"] == sorted([me, other])

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
    assert opened["change_request"]["reviewers"] == sorted([me, other])
    request_id = opened["id"]

    reviewer = support.connect(agent_id=REVIEWER)
    try:
        approved = support.stewardship(
            reviewer, graph, {"op": "review", "request_id": request_id, "approve": True}
        )
    finally:
        reviewer.close()
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


@pytest.mark.spec("MEDULLA-TYPED-PACKS-R802.4")
def test_stewardship_refusals(served) -> None:
    """Verifies MEDULLA-TYPED-PACKS-R802.4: each code is refused and the request is unchanged."""
    graph, client = served
    me = request_context()["principal"]
    support.assign_stewards(client, graph, ASSET, me, support.principal(REVIEWER))
    request_id = support.open_request(client, graph, ASSET)
    stranger = support.connect(agent_id=STRANGER)
    reviewer = support.connect(agent_id=REVIEWER)
    try:
        with pytest.raises(Exception, match="STEWARD_ROLE_MISSING"):
            support.stewardship(
                stranger, graph, {"op": "review", "request_id": request_id, "approve": True}
            )
        with pytest.raises(Exception, match="SELF_APPROVAL_DENIED"):
            _op(client, graph, {"op": "review", "request_id": request_id, "approve": True})
        # The target moves after the request was opened: the review is stale.
        client._send(
            "AddNode",
            {
                "node_id": ASSET,
                "properties_msgpack": msgpack.packb(
                    {"type": "Column", "content_digest": "sha256:" + "cd" * 32},
                    use_bin_type=True,
                ),
            },
            graph,
        )
        with pytest.raises(Exception, match="CHANGE_REQUEST_STALE"):
            support.stewardship(
                reviewer, graph, {"op": "review", "request_id": request_id, "approve": True}
            )
    finally:
        stranger.close()
        reviewer.close()
    stored = _decode(client._send("GetNodeProperties", {"node_id": request_id}, graph))
    assert stored["state"] == "open"


@pytest.mark.spec("MEDULLA-TYPED-PACKS-R802.5")
def test_request_evidence_reads_back(served) -> None:
    """Verifies MEDULLA-TYPED-PACKS-R802.5: digests, verdicts and the receipt are typed properties."""
    graph, client = served
    reviewer = support.connect(agent_id=REVIEWER)
    try:
        request_id = support.approved_request(client, reviewer, graph, ASSET)
    finally:
        reviewer.close()
    _op(client, graph, {"op": "apply", "request_id": request_id})
    stored = _decode(client._send("GetNodeProperties", {"node_id": request_id}, graph))
    assert stored["state"] == "applied"
    assert stored["digest_after"] == DIGEST
    assert "digest_before" in stored
    assert stored["receipt_id"] == f"receipt:{request_id}"
    assert [verdict["approve"] for verdict in stored["verdicts"]] == [True]
    assert stored["verdicts"][0]["principal"] == support.principal(REVIEWER)
    assert stored["applied_by"] == request_context()["principal"]


@pytest.mark.spec("MEDULLA-TYPED-PACKS-R802.6")
def test_change_request_journey_is_tenant_scoped(served) -> None:
    """Verifies MEDULLA-TYPED-PACKS-R802.6: the served journey reads back, and a second tenant sees nothing."""
    from conftest import tenant_request_context

    graph, client = served
    reviewer = support.connect(agent_id=REVIEWER)
    try:
        request_id = support.approved_request(client, reviewer, graph, ASSET)
    finally:
        reviewer.close()
    _op(client, graph, {"op": "apply", "request_id": request_id})
    nodes = _decode(
        client._send("GetNodesByLabel", {"label": "cat:ChangeRequest", "after": None, "limit": 10}, graph)
    )
    assert [node_id for node_id, _ in nodes] == [request_id]

    foreign = SyncMedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        graph_name=graph,
        verified_context=tenant_request_context("tenant:foreign"),
    )
    try:
        # The second tenant resolves no reviewers: it sees no assignment.
        with pytest.raises(Exception, match="STEWARD_ROLE_MISSING|denied|DENIED|TENANT"):
            support.stewardship(
                foreign,
                graph,
                {
                    "op": "open_change_request",
                    "target": ASSET,
                    "proposed_digest": DIGEST,
                    "scopes": {"asset": ASSET},
                },
            )
    finally:
        foreign.close()


@pytest.mark.spec("MEDULLA-TYPED-PACKS-R802.8")
def test_critical_element_gaps_reported(served) -> None:
    """Verifies MEDULLA-TYPED-PACKS-R802.8: a critical element lists what it misses."""
    graph, client = served
    # A principal that governs nothing cannot mark an element critical.
    with pytest.raises(Exception, match="STEWARD_ROLE_MISSING"):
        _op(client, graph, {"op": "mark_critical", "element": ASSET})
    support.assign_stewards(client, graph, ASSET, support.principal())
    marked = _op(client, graph, {"op": "mark_critical", "element": ASSET})
    node = _decode(client._send("GetNodeProperties", {"node_id": marked["id"]}, graph))
    # A steward is assigned; no owner and no active quality rule exist yet.
    assert node["gaps"] == "owner,quality_rule"
    assert node["element"] == ASSET
    rule = {
        "id": "total-present",
        "target": ASSET,
        "dimension": "completeness",
        "expression": "total IS NOT NULL",
        "threshold": 0.9,
        "severity": "warning",
    }
    _decode(client._send("QualityRulePut", {"rule": rule}, graph))
    _op(
        client,
        graph,
        {
            "op": "assign",
            "assignment": {
                "scope": {"kind": "asset", "id": ASSET},
                "role": "owner",
                "principal": support.principal(),
            },
        },
    )
    _op(client, graph, {"op": "mark_critical", "element": ASSET})
    node = _decode(client._send("GetNodeProperties", {"node_id": marked["id"]}, graph))
    assert node["gaps"] == ""
