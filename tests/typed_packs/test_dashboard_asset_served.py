"""Served dashboard catalog asset (MEDULLA-TYPED-PACKS-R109).

Drives `DashboardAsset` through the real served surface as three registered agents: the owner,
a member of a granted group and a stranger.
"""

from __future__ import annotations

import os
import uuid
from typing import Any

import pytest
import stewardship_support as support
from conftest import request_context

from medulla.client import SyncMedullaClient

OWNER = "agent-dashboard-owner"
MEMBER = "agent-dashboard-member"
STRANGER = "agent-dashboard-stranger"
GROUP = "group:sales"
DASH = "orders-overview"
CAT = "https://knuckles-team.github.io/ontology/catalog#"


def _connect(graph: str | None, agent_id: str, roles: list[str] | None = None) -> Any:
    return SyncMedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        graph_name=graph,
        verified_context=request_context(agent_id=agent_id, roles=roles),
    )


def _dash(client: Any, graph: str, op: dict[str, Any]) -> dict[str, Any]:
    return support.decode(client._send("DashboardAsset", {"op": op}, graph))


def _body(*sources: str) -> dict[str, Any]:
    return {
        "panels": [
            {"id": f"p{n}", "source": source, "query": f"SELECT * FROM {source}"}
            for n, source in enumerate(sources)
        ],
        "layout": {"columns": 2},
        "parameters": {},
    }


def _save(client: Any, graph: str, revision: int, *sources: str) -> dict[str, Any]:
    return _dash(
        client, graph, {"op": "save", "id": DASH, "body": _body(*sources), "revision": revision}
    )


def _node(client: Any, graph: str, node_id: str) -> dict[str, Any] | None:
    return support.decode(client._send("GetNodeProperties", {"node_id": node_id}, graph))


@pytest.fixture
def served():
    graph = f"medulla-r109-{uuid.uuid4().hex}"
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


@pytest.mark.spec("MEDULLA-TYPED-PACKS-R109.3")
def test_save_share_delete_restore_reach_the_handler_with_grant_checks(served):
    # spec: MEDULLA-TYPED-PACKS-R109.3
    graph, owner, member, stranger = served
    saved = _save(owner, graph, 0, "orders")
    assert saved["revision"] == 1
    assert saved["owner"] == support.principal(OWNER)

    # Nobody else reads it until it is shared.
    with pytest.raises(Exception, match="DASHBOARD_NOT_GRANTED"):
        _dash(member, graph, {"op": "get", "id": DASH})
    shared = _dash(
        owner,
        graph,
        {"op": "share", "id": DASH, "grants": [{"principal_or_group": GROUP, "role": "view"}]},
    )
    assert shared["grants"][0]["principal_or_group"] == GROUP
    assert _dash(member, graph, {"op": "get", "id": DASH})["body"]["panels"][0]["source"] == "orders"
    with pytest.raises(Exception, match="DASHBOARD_NOT_GRANTED"):
        _dash(stranger, graph, {"op": "get", "id": DASH})

    # A viewer neither saves, shares, deletes nor restores.
    with pytest.raises(Exception, match="DASHBOARD_NOT_GRANTED"):
        _save(member, graph, 1, "orders")
    for op in ("delete", "restore"):
        with pytest.raises(Exception, match="DASHBOARD_NOT_GRANTED"):
            _dash(member, graph, {"op": op, "id": DASH})

    # The owner deletes; the body stays and a restore brings it back.
    deleted = _dash(owner, graph, {"op": "delete", "id": DASH})
    assert deleted["removed_at_ms"] > 0
    with pytest.raises(Exception, match="DASHBOARD_NOT_GRANTED"):
        _dash(member, graph, {"op": "get", "id": DASH})
    restored = _dash(owner, graph, {"op": "restore", "id": DASH})
    assert "removed_at_ms" not in restored or restored["removed_at_ms"] is None


@pytest.mark.spec("MEDULLA-TYPED-PACKS-R109.4")
def test_stale_revision_missing_grant_invalid_body_and_purged_asset_refuse(served):
    # spec: MEDULLA-TYPED-PACKS-R109.4
    graph, owner, member, stranger = served
    _save(owner, graph, 0, "orders")
    _save(owner, graph, 1, "orders", "refunds")

    # A stale revision names the current one and overwrites nothing.
    with pytest.raises(Exception, match="DASHBOARD_REVISION_CONFLICT.*revision 2"):
        _save(owner, graph, 1, "orders")
    node = _node(owner, graph, f"dashboard:{DASH}")
    assert node["revision"] == 2 and "refunds" in node["body"]

    # A stranger has no grant; the refusal is the same whether the id exists or not.
    with pytest.raises(Exception, match="DASHBOARD_NOT_GRANTED"):
        _save(stranger, graph, 2, "orders")
    with pytest.raises(Exception, match="DASHBOARD_NOT_GRANTED"):
        _dash(stranger, graph, {"op": "get", "id": "no-such-dashboard"})

    # A panel with no query, and a body over the knob, are invalid.
    with pytest.raises(Exception, match="DASHBOARD_BODY_INVALID"):
        _dash(
            owner,
            graph,
            {
                "op": "save",
                "id": DASH,
                "revision": 2,
                "body": {"panels": [{"id": "p", "source": "orders", "query": " "}]},
            },
        )
    huge = _body("orders")
    huge["parameters"] = {"blob": "x" * (2 * 1024 * 1024)}
    with pytest.raises(Exception, match="DASHBOARD_BODY_INVALID"):
        _dash(owner, graph, {"op": "save", "id": DASH, "body": huge, "revision": 2})
    assert _node(owner, graph, f"dashboard:{DASH}")["revision"] == 2
    # The purged read is proved on the real tick: `dashboard_purge_tick_tests` in
    # `crates/medulla-h-jobs/src/jobs/schedule_tick.rs` drives `tick_pass` over a served state.


@pytest.mark.spec("MEDULLA-TYPED-PACKS-R109.5")
def test_revision_grant_and_delete_history_and_usage_are_served(served):
    # spec: MEDULLA-TYPED-PACKS-R109.5
    graph, owner, member, _ = served
    _save(owner, graph, 0, "orders")
    _save(owner, graph, 1, "orders")
    first = _node(owner, graph, f"dashboard_revision:{DASH}:1")
    second = _node(owner, graph, f"dashboard_revision:{DASH}:2")
    assert (first["revision"], first["prior_revision"]) == (1, 0)
    assert (second["revision"], second["prior_revision"]) == (2, 1)
    assert second["saved_by"] == support.principal(OWNER) and second["saved_at_ms"] > 0
    assert second["body_sha256"].startswith("sha256:") and second["body_sha256"] != first["body_sha256"]

    _dash(
        owner,
        graph,
        {"op": "share", "id": DASH, "grants": [{"principal_or_group": GROUP, "role": "view"}]},
    )
    asset = _node(owner, graph, f"dashboard:{DASH}")
    assert asset["grants_by"] == support.principal(OWNER) and asset["grants_at_ms"] > 0

    # Each read through the handler is counted.
    _dash(member, graph, {"op": "get", "id": DASH})
    read = _dash(member, graph, {"op": "get", "id": DASH})
    assert read["usage_count"] == 2 and read["last_viewed_ms"] > 0

    # A delete records when the retention ends.
    _dash(owner, graph, {"op": "delete", "id": DASH})
    asset = _node(owner, graph, f"dashboard:{DASH}")
    assert asset["retention_ends_ms"] > asset["removed_at_ms"]


@pytest.mark.spec("MEDULLA-TYPED-PACKS-R109")
@pytest.mark.spec("MEDULLA-TYPED-PACKS-R109.6")
def test_dashboard_save_conflict_share_lineage_search_and_soft_delete(served):
    # spec: MEDULLA-TYPED-PACKS-R109.6
    graph, owner, member, stranger = served
    for source in ("orders", "refunds"):
        owner.rdf.add_triples(
            turtle=(
                f"<{source}> <http://www.w3.org/2004/02/skos/core#prefLabel> \"{source}\" .\n"
            )
        )
    first = _save(owner, graph, 0, "orders", "refunds")
    with pytest.raises(Exception, match="DASHBOARD_REVISION_CONFLICT"):
        _save(owner, graph, 0, "orders")

    _dash(
        owner,
        graph,
        {"op": "share", "id": DASH, "grants": [{"principal_or_group": GROUP, "role": "view"}]},
    )
    assert _dash(member, graph, {"op": "get", "id": DASH})["revision"] == first["revision"]
    with pytest.raises(Exception, match="DASHBOARD_NOT_GRANTED"):
        _dash(stranger, graph, {"op": "get", "id": DASH})

    # The lineage of a panel reads back to its source.
    rows = owner.rdf.sparql(
        f"SELECT ?source WHERE {{ ?source <{CAT}lineageTo> <dashboard:{DASH}> }}"
    )
    assert {row["source"].strip("<>") for row in rows} == {"orders", "refunds"}

    # The dashboard is found by search, leaves it on delete and returns on restore.
    search = (
        f"PREFIX cat: <{CAT}> "
        "PREFIX skos: <http://www.w3.org/2004/02/skos/core#> "
        "SELECT ?asset ?score WHERE { ?asset skos:prefLabel ?name . "
        f'BIND(cat:relevance(?asset, "{DASH}") AS ?score) FILTER(?score > 0) }}'
    )
    found = lambda: {row["asset"].strip("<>") for row in owner.rdf.sparql(search)}  # noqa: E731
    assert f"dashboard:{DASH}" in found()
    _dash(owner, graph, {"op": "delete", "id": DASH})
    assert f"dashboard:{DASH}" not in found()
    _dash(owner, graph, {"op": "restore", "id": DASH})
    assert f"dashboard:{DASH}" in found()
