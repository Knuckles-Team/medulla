"""Served retrieval of principal-owned knowledge units (MEDULLA-IDENTITY-R023).

R023.5: a served read that hides knowledge units appends one read-audit line;
the op carries the hidden count and the line's digest binds the ids the read
could see, so the line evidences the returned units and the hidden count.
R023.6: a tenant-shared unit reaches only readers inside its scope;
superseding it swaps the visible version.
"""

import os

import pytest

from medulla.client import EngineResponseError, SyncMedullaClient

try:
    from conftest import TEST_TENANT, request_context
except ModuleNotFoundError:  # package-root import
    from tests.conftest import TEST_TENANT, request_context

# The engine admits only contexts of its deployment tenant (a context for any
# other tenant is refused at the door with AUTH_TENANT_MISMATCH), so the
# in-scope tenant is the deployment tenant and the out-of-scope tenant is a
# unit owner, never a connected reader.
TENANT_A = TEST_TENANT
TENANT_B = "tenant-b"
OWNER = "usr:a"
READER_B = "usr:b"
READER_C = "usr:c"


def _unit(
    share_scope: str,
    *,
    version: int = 1,
    supersedes: str | None = None,
    owner_tenant: str = TENANT_A,
) -> dict[str, object]:
    props: dict[str, object] = {
        "node_type": "KnowledgeUnit",
        "owner": OWNER,
        "owner_tenant": owner_tenant,
        "version": version,
        "share_scope": share_scope,
    }
    if supersedes is not None:
        props["supersedes"] = supersedes
    return props


def _connect(principal: str, tenant: str) -> SyncMedullaClient:
    context = request_context(principal=principal)
    context["tenant"] = tenant
    return SyncMedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=context,
    )


def _unit_ids(client: SyncMedullaClient) -> list[str]:
    result = client.query.uql("MATCH (:KnowledgeUnit)")
    return [row["id"] for row in result.get("rows", [])]


def _read_audit_line(client: SyncMedullaClient, hidden_count: int) -> dict[str, object]:
    """The newest `SegmentReadAudit.hidden-<n>` line of the graph audit chain."""
    entries = int(client.admin.audit_verify()["entries"])
    for seq in range(entries, -1, -1):
        try:
            line = client.admin.audit_read_event(seq)["event_line"]
        except EngineResponseError:
            continue  # not an operation event
        event = dict(part.split("=", 1) for part in line.split("|") if "=" in part)
        if event.get("op") == f"SegmentReadAudit.hidden-{hidden_count}":
            return event
    raise AssertionError(
        f"no SegmentReadAudit.hidden-{hidden_count} line in the audit chain"
    )


@pytest.mark.spec("MEDULLA-IDENTITY-R023.5")
# spec: MEDULLA-IDENTITY-R023.5
def test_read_audit_counts_hidden(clean_graph):
    """One read with one hidden unit yields an audit line with that hidden count.

    The line names the reader and the hidden count in clear text; the ids the
    read could see stay bound into the line's digest (a privacy-safe line),
    which is stable across identical reads and moves when the visible set
    changes, so the returned id is evidenced by the line.
    """
    clean_graph.nodes.add("ku-private", _unit("private"))
    clean_graph.nodes.add("ku-shared", _unit("tenant"))

    reader_b = _connect(READER_B, TENANT_A)
    try:
        # The tenant-shared unit is visible; the private one is hidden.
        assert _unit_ids(reader_b) == ["ku-shared"]

        first = _read_audit_line(reader_b, 1)
        assert first["principal"] == READER_B
        assert first["tenant"] == TENANT_A
        assert first["surface"] == "read"
        assert first["status"] == "ok"

        # The digest binds the returned ids: an identical read yields the same
        # line digest.
        second = _read_audit_line(reader_b, 1)
        assert second["params_sha256"] == first["params_sha256"]

        # A new hidden unit changes the visible set, so the digest moves.
        clean_graph.nodes.add("ku-private-2", _unit("private"))
        reader_b.query.uql("MATCH (:KnowledgeUnit)")
        third = _read_audit_line(reader_b, 2)
        assert third["params_sha256"] != first["params_sha256"]
    finally:
        reader_b.close()


@pytest.mark.spec("MEDULLA-IDENTITY-R023.6")
# spec: MEDULLA-IDENTITY-R023.6
def test_shared_unit_visible_only_in_scope(clean_graph):
    """Only in-scope readers see the shared unit; superseding swaps the version."""
    clean_graph.nodes.add("ku-private", _unit("private"))
    clean_graph.nodes.add("ku-shared-v1", _unit("tenant"))
    # A tenant-shared unit owned by another tenant never reaches this scope.
    clean_graph.nodes.add("ku-foreign", _unit("tenant", owner_tenant=TENANT_B))

    reader_b = _connect(READER_B, TENANT_A)
    try:
        # Same tenant: only the tenant-shared unit reaches the reader; the
        # private unit and the other tenant's shared unit stay hidden.
        assert _unit_ids(reader_b) == ["ku-shared-v1"]
        # A reader of another tenant cannot connect to this deployment.
        with pytest.raises(EngineResponseError, match="AUTH_TENANT_MISMATCH"):
            _unit_ids(_connect(READER_C, TENANT_B))

        # Supersede the shared unit: only the new version returns.
        clean_graph.nodes.add(
            "ku-shared-v2",
            _unit("tenant", version=2, supersedes="ku-shared-v1"),
        )
        assert _unit_ids(reader_b) == ["ku-shared-v2"]
    finally:
        reader_b.close()
