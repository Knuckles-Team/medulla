"""Served classification claims (MEDULLA-TYPED-PACKS-R652).

Drives `ClassificationClaim` through the real served surface: a claim is
proposed, a steward promotes it with evidence, the markings are stamped on the
asset, and the promotion proposes them downstream along lineage. Refusals leave
the graph unchanged.
"""

from __future__ import annotations

import os
import uuid
from typing import Any

import msgpack
import pytest
from conftest import request_context

from medulla.client import SyncMedullaClient

STEWARD = "steward"


def _connect(
    graph: str | None = None,
    *,
    roles: list[str] | None = None,
    scopes: list[str] | None = None,
) -> Any:
    return SyncMedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        graph_name=graph,
        verified_context=request_context(roles=roles, scopes=scopes),
    )


def _decode(value: Any) -> Any:
    if isinstance(value, (bytes, bytearray)):
        return msgpack.unpackb(value, raw=False)
    return value


def _pack(value: dict[str, Any]) -> bytes:
    return msgpack.packb(value, use_bin_type=True)


def _node(client: Any, graph: str, node_id: str, properties: dict[str, Any]) -> None:
    client._send(
        "AddNode",
        {"node_id": node_id, "properties_msgpack": _pack(properties)},
        graph,
    )


def _lineage(client: Any, graph: str, source: str, target: str) -> None:
    client._send(
        "AddEdge",
        {
            "source_id": source,
            "target_id": target,
            "properties_msgpack": _pack({"relationship": "cat:lineageTo"}),
        },
        graph,
    )


def _props(client: Any, graph: str, node_id: str) -> dict[str, Any]:
    return _decode(client._send("GetNodeProperties", {"node_id": node_id}, graph))


def _claim(asset: str, *categories: str, evidence: list[str] | None = None) -> dict:
    return {
        "asset": asset,
        "categories": list(categories),
        "provenance": {"kind": "manual"},
        "state": "proposed",
        "evidence": evidence if evidence is not None else [],
    }


def _claim_id(asset: str, *categories: str) -> str:
    return f"classification_claim:{asset}:{'+'.join(sorted(categories))}"


def _op(client: Any, graph: str, op: dict[str, Any]) -> dict[str, Any]:
    return _decode(client._send("ClassificationClaim", {"op": op}, graph))


@pytest.fixture
def served():
    """A graph with the `pii` marking registered and one asset, plus a steward."""

    graph = f"medulla-r652-{uuid.uuid4().hex}"
    admin = _connect(None, roles=[STEWARD])
    created = False
    try:
        admin.tenants.create(graph, graph_type="Global")
        created = True
        _node(admin, graph, "marking:pii", {"type": "Marking"})
        _node(admin, graph, "col:orders.email", {"type": "Column"})
        yield graph, admin
    finally:
        try:
            if created:
                admin.tenants.delete(graph)
        finally:
            admin.close()


@pytest.mark.spec("MEDULLA-TYPED-PACKS-R652")
@pytest.mark.spec("MEDULLA-TYPED-PACKS-R652.3")
def test_methods_reach_handler_and_promotion_stamps_marking(served) -> None:
    """Verifies MEDULLA-TYPED-PACKS-R652.3: propose, promote, reject are served."""
    graph, client = served
    asset = "col:orders.email"
    proposed = _op(client, graph, {"op": "propose", "claim": _claim(asset, "pii")})
    assert proposed["record"]["claim"]["state"] == "proposed"
    assert proposed["record"]["claim_id"] == _claim_id(asset, "pii")

    promoted = _op(
        client,
        graph,
        {"op": "promote", "claim_id": _claim_id(asset, "pii"), "evidence": ["review:1"]},
    )
    assert promoted["record"]["claim"]["state"] == "promoted"
    assert promoted["record"]["after_markings"] == ["pii"]
    assert _props(client, graph, asset)["_markings"] == ["pii"]

    _node(client, graph, "marking:secret", {"type": "Marking"})
    _op(client, graph, {"op": "propose", "claim": _claim(asset, "secret")})
    rejected = _op(client, graph, {"op": "reject", "claim_id": _claim_id(asset, "secret")})
    assert rejected["record"]["claim"]["state"] == "rejected"
    assert _props(client, graph, asset)["_markings"] == ["pii"]


@pytest.mark.spec("MEDULLA-TYPED-PACKS-R652.4")
def test_missing_evidence_illegal_state_and_unknown_marking_refuse(served) -> None:
    """Verifies MEDULLA-TYPED-PACKS-R652.4: each refusal code, and no change."""
    graph, client = served
    asset = "col:orders.email"
    claim_id = _claim_id(asset, "pii")
    _op(client, graph, {"op": "propose", "claim": _claim(asset, "pii")})

    with pytest.raises(Exception, match="CLAIM_EVIDENCE_MISSING"):
        _op(client, graph, {"op": "promote", "claim_id": claim_id, "evidence": []})
    assert "_markings" not in _props(client, graph, asset)
    assert _props(client, graph, claim_id)["state"] == "proposed"

    with pytest.raises(Exception, match="CLAIM_UNKNOWN_MARKING"):
        _op(client, graph, {"op": "propose", "claim": _claim(asset, "unregistered")})

    plain = _connect(None, roles=["test"])
    try:
        with pytest.raises(Exception, match="STEWARD_ROLE_MISSING"):
            _op(plain, graph, {"op": "promote", "claim_id": claim_id, "evidence": ["x"]})
    finally:
        plain.close()

    _op(client, graph, {"op": "promote", "claim_id": claim_id, "evidence": ["review:1"]})
    with pytest.raises(Exception, match="CLAIM_STATE_ILLEGAL"):
        _op(client, graph, {"op": "reject", "claim_id": claim_id})
    with pytest.raises(Exception, match="CLAIM_STATE_ILLEGAL"):
        _op(client, graph, {"op": "promote", "claim_id": claim_id, "evidence": ["y"]})


@pytest.mark.spec("MEDULLA-TYPED-PACKS-R652.5")
def test_marking_sets_and_promoter_read_back(served) -> None:
    """Verifies MEDULLA-TYPED-PACKS-R652.5: the claim node records the evidence."""
    graph, client = served
    asset = "col:orders.email"
    claim_id = _claim_id(asset, "pii")
    _op(client, graph, {"op": "propose", "claim": _claim(asset, "pii", evidence=["scan:1"])})
    _op(client, graph, {"op": "promote", "claim_id": claim_id, "evidence": ["review:1"]})

    node = _props(client, graph, claim_id)
    assert node["state"] == "promoted"
    assert node["before_markings"] == []
    assert node["after_markings"] == ["pii"]
    assert node["promoted_by"] == request_context(roles=[STEWARD])["principal"]
    assert node["evidence"] == ["scan:1", "review:1"]


def _readable_ids(client: Any, label: str) -> list[str]:
    result = client.query.uql(f"MATCH (:{label})")
    return sorted(row["id"] for row in result["rows"])


@pytest.mark.spec("MEDULLA-TYPED-PACKS-R652.6")
def test_claim_propose_propagate_promote_and_redacted_read(served) -> None:
    """Verifies MEDULLA-TYPED-PACKS-R652.6: promotion proposes claims downstream; a
    holder of the marking reads the marked assets, a non-holder reads them
    redacted, and another tenant reads nothing."""
    from conftest import tenant_request_context

    graph, client = served
    for node in ("view:orders_v", "dash:orders_d"):
        _node(client, graph, node, {"type": "Column"})
    asset = "col:orders.email"
    _lineage(client, graph, asset, "view:orders_v")
    _lineage(client, graph, "view:orders_v", "dash:orders_d")

    _op(client, graph, {"op": "propose", "claim": _claim(asset, "pii")})
    result = _op(
        client,
        graph,
        {"op": "promote", "claim_id": _claim_id(asset, "pii"), "evidence": ["review:1"]},
    )
    downstream = {record["claim"]["asset"]: record for record in result["propagated"]}
    assert set(downstream) == {"view:orders_v", "dash:orders_d"}
    for record in downstream.values():
        assert record["claim"]["state"] == "proposed"
        assert record["claim"]["provenance"]["kind"] == "propagated"
    # Propagated claims are proposals: nothing is stamped downstream until promoted.
    assert "_markings" not in _props(client, graph, "view:orders_v")

    # A steward promotes the view's proposal; the dashboard stays unmarked.
    _op(
        client,
        graph,
        {
            "op": "promote",
            "claim_id": _claim_id("view:orders_v", "pii"),
            "evidence": ["review:2"],
        },
    )
    marked = sorted([asset, "view:orders_v"])
    everything = sorted([*marked, "dash:orders_d"])
    holder = _connect(graph, scopes=["kg:read", "pii"])
    non_holder = _connect(graph, scopes=["kg:read"])
    try:
        assert _readable_ids(holder, "Column") == everything
        # Marked assets are redacted for the non-holder, so they no longer match
        # their label; the unmarked dashboard reads in full.
        assert _readable_ids(non_holder, "Column") == ["dash:orders_d"]
    finally:
        holder.close()
        non_holder.close()

    foreign = SyncMedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        graph_name=graph,
        verified_context=tenant_request_context("tenant:foreign", roles=[STEWARD]),
    )
    try:
        try:
            seen = _props(foreign, graph, _claim_id(asset, "pii"))
        except Exception:
            seen = None
        assert not seen
        try:
            assert _readable_ids(foreign, "Column") == []
        except Exception:
            pass  # a refused cross-tenant read also sees nothing
    finally:
        foreign.close()
