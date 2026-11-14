"""Served tenant scope of graph reads (MEDULLA-IDENTITY-R610).

R610.3: every served read entry point (`Uql`, `UnifiedQuery`, `Sql`) is narrowed to
the verified tenant's own graph, the shared graphs of its ancestors and the
commons, and nothing else.
R610.6: a three-level tenant chain is read by its leaf; the visibility order,
the one audit line per read and the refusal of an unscopable query hold on the
served path.

A deployed engine admits exactly one tenant: a request context naming any other
tenant is refused at the door with AUTH_TENANT_MISMATCH. So the connected reader
is always the deployment tenant, placed as the leaf of the chain, and the other
tenants are only owners of stored nodes. Reading the same store as each of
several tenants is proven at the read chokepoint, in
`crates/medulla-h-query/src/query/planning.rs::tenant_scope_served_tests`.
"""

import os
import time

import pytest

from medulla.client import EngineResponseError, SyncMedullaClient

try:
    from conftest import TEST_TENANT, request_context
except ModuleNotFoundError:  # package-root import
    from tests.conftest import TEST_TENANT, request_context

ROOT = "tenant-root"
MID = "tenant-mid"
LEAF = TEST_TENANT  # the one tenant the engine admits
OTHER = "tenant-other"

# The root shares its graph with descendants; the middle tenant keeps its graph private.
CHAIN_LINKS = {
    "link-mid": {
        "node_type": "TenantParent",
        "tenant": MID,
        "parent": ROOT,
        "shared": True,
    },
    "link-leaf": {"node_type": "TenantParent", "tenant": LEAF, "parent": MID},
}
NOTES = {
    "note-root": {"node_type": "Note", "_tenant": ROOT},
    "note-mid": {"node_type": "Note", "_tenant": MID},
    "note-leaf": {"node_type": "Note", "_tenant": LEAF},
    "note-other": {"node_type": "Note", "_tenant": OTHER},
    "note-commons": {"node_type": "Note"},
}

# What the leaf reads: own graph, the sharing root's graph, the commons; the
# private middle graph and the foreign tenant's graph stay hidden.
VISIBLE_TO_LEAF = {"note-leaf", "note-root", "note-commons"}


def _connect(tenant: str) -> SyncMedullaClient:
    context = request_context(principal=f"usr:{tenant}")
    context["tenant"] = tenant
    return SyncMedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=context,
    )


def _seed(clean_graph) -> None:
    for node_id, props in {**CHAIN_LINKS, **NOTES}.items():
        clean_graph.nodes.add(node_id, props)


def _uql_notes(client: SyncMedullaClient) -> set[str]:
    return {row["id"] for row in client.query.uql("MATCH (:Note)").get("rows", [])}


def _unified_notes(client: SyncMedullaClient) -> set[str]:
    rows = client.query.unified([{"Scan": {"label": "Note"}}])
    return {row["id"] for row in rows}


def _sql_notes(client: SyncMedullaClient) -> set[str]:
    rows = client.query.sql("SELECT id FROM nodes WHERE type = 'Note'")
    return {row["id"] for row in rows}


def _audit_lines(client: SyncMedullaClient) -> int:
    return int(client.admin.audit_verify()["entries"])


def _audit_lines_after(client: SyncMedullaClient, expected: int) -> int:
    """The audit queue drains asynchronously; wait for the expected count."""
    deadline = time.monotonic() + 10
    count = _audit_lines(client)
    while count < expected and time.monotonic() < deadline:
        time.sleep(0.05)
        count = _audit_lines(client)
    return count


@pytest.mark.spec("MEDULLA-IDENTITY-R610.3")
# spec: MEDULLA-IDENTITY-R610.3
def test_scope_predicate_injected_on_served_read(clean_graph):
    """Each entry point returns only own, shared-ancestor and commons notes.

    A context naming a foreign tenant is refused at the door.
    """
    _seed(clean_graph)
    client = _connect(LEAF)
    try:
        assert _uql_notes(client) == VISIBLE_TO_LEAF
        assert _unified_notes(client) == VISIBLE_TO_LEAF
        assert _sql_notes(client) == VISIBLE_TO_LEAF
    finally:
        client.close()
    foreign = _connect(OTHER)
    try:
        with pytest.raises(EngineResponseError, match="AUTH_TENANT_MISMATCH"):
            _uql_notes(foreign)
    finally:
        foreign.close()


@pytest.mark.spec("MEDULLA-IDENTITY-R610.6")
# spec: MEDULLA-IDENTITY-R610.6
def test_chain_visibility_and_unscopable_refusal(clean_graph):
    """A three-level chain reads in order, audits each read, refuses unscopable."""
    _seed(clean_graph)
    leaf = _connect(LEAF)
    try:
        # Visibility order: own graph, the sharing ancestor, then the commons;
        # the private middle graph and the foreign tenant never appear.
        assert _uql_notes(leaf) == VISIBLE_TO_LEAF

        # One audit line per read.
        before = _audit_lines(leaf)
        _uql_notes(leaf)
        assert _audit_lines_after(leaf, before + 1) == before + 1
        _uql_notes(leaf)
        assert _audit_lines_after(leaf, before + 2) == before + 2

        # A plan binding no node variable is refused, runs nothing, is not audited.
        settled = _audit_lines(leaf)
        with pytest.raises((EngineResponseError, RuntimeError)) as refused:
            leaf.query.unified([])
        assert "QUERY_UNSCOPABLE" in str(refused.value)
        time.sleep(0.3)
        assert _audit_lines(leaf) == settled
        # The graph is unchanged: the same read returns the same visible set.
        assert _uql_notes(leaf) == VISIBLE_TO_LEAF
    finally:
        leaf.close()
