"""Served node sharing (MEDULLA-IDENTITY-R023.13).

R023.13.4: a share refused for authority, tenant or an absent node changes no state.
R023.13.6: a node shared to the tenant and then revoked is invisible to a second
principal after the engine restarts on the same data directory.
"""

import contextlib
import os
from pathlib import Path

import pytest

from medulla.client import EngineResponseError, SyncMedullaClient

try:
    from conftest import (
        TEST_AGENT_ID,
        TEST_SIGNER_KEY,
        _prebuilt_test_binary,
        bootstrap_context,
        register_commons_agents,
        request_context,
        spawn_server,
        stop_server,
        strict_server_env,
        tenant_request_context,
        wait_for_server,
    )
except ModuleNotFoundError:  # package-root import
    from tests.conftest import (
        TEST_AGENT_ID,
        TEST_SIGNER_KEY,
        _prebuilt_test_binary,
        bootstrap_context,
        register_commons_agents,
        request_context,
        spawn_server,
        stop_server,
        strict_server_env,
        tenant_request_context,
        wait_for_server,
    )

OWNER = "usr:share-owner"
OTHER = "usr:share-other"
AUTH_SECRET = "test-medulla-secret"


def _connect(socket_path: str, principal: str) -> SyncMedullaClient:
    return SyncMedullaClient.connect(
        socket_path=socket_path,
        auth_secret=AUTH_SECRET,
        verified_context=request_context(agent_id=principal),
    )


def _note(owner: str) -> dict[str, object]:
    return {"node_type": "Note", "_owner_id": owner}


def _note_ids(client: SyncMedullaClient) -> list[str]:
    return [row["id"] for row in client.query.uql("MATCH (:Note)").get("rows", [])]


@pytest.mark.spec("MEDULLA-IDENTITY-R023.13.4")
def test_share_refusals_change_nothing(clean_graph):
    """A non-owner, a foreign tenant and an absent node are each refused with their
    code, and an independent read of the node's share keys is unchanged."""
    register_commons_agents(OWNER, OTHER)
    socket_path = os.environ["GRAPH_SERVICE_SOCKET"]
    owner = _connect(socket_path, OWNER)
    other = _connect(socket_path, OTHER)
    foreign = SyncMedullaClient.connect(
        socket_path=socket_path,
        auth_secret=AUTH_SECRET,
        verified_context=tenant_request_context("tenant-b", agent_id=OTHER),
    )
    try:
        owner.nodes.add("share-refusal-1", _note(OWNER))
        assert owner.rbac.share_grant("share-refusal-1", "tenant") == "share_granted"
        before = owner.nodes.properties("share-refusal-1")

        with pytest.raises(EngineResponseError, match="ACCESS_DENIED"):
            other.rbac.share_revoke("share-refusal-1")
        with pytest.raises(EngineResponseError, match="AUTH_TENANT_MISMATCH"):
            foreign.rbac.share_revoke("share-refusal-1")
        with pytest.raises(EngineResponseError, match="INVALID_ARGUMENT"):
            owner.rbac.share_revoke("share-refusal-absent")
        with pytest.raises(EngineResponseError):
            owner.rbac.share_grant("share-refusal-1", "placement")

        # A private node the caller cannot see answers exactly as an absent node
        # does, so the refusal does not reveal that the node exists.
        owner.nodes.add("share-refusal-hidden", {**_note(OWNER), "_shared_scope": "private"})
        with pytest.raises(EngineResponseError) as absent:
            other.rbac.share_revoke("share-refusal-absent")
        with pytest.raises(EngineResponseError) as hidden:
            other.rbac.share_revoke("share-refusal-hidden")
        assert "INVALID_ARGUMENT" in str(hidden.value)
        assert str(hidden.value).replace("share-refusal-hidden", "X") == str(
            absent.value
        ).replace("share-refusal-absent", "X")

        assert owner.nodes.properties("share-refusal-1") == before
    finally:
        for client in (owner, other, foreign):
            client.close()


@contextlib.contextmanager
def _engine(root: Path):
    """One engine on `root`'s data directory, bootstrapped once; leaving the block
    stops it with SIGTERM so the next call restarts it on the same data."""
    socket_path = str(root / "engine.sock")
    log_path = root / "engine.log"
    persist_dir = root / "persist"
    persist_dir.mkdir(parents=True, exist_ok=True)
    env = strict_server_env(
        str(root / "security"), auth_secret=AUTH_SECRET, persist_dir=str(persist_dir)
    )
    env.setdefault("MEDULLA_ENCRYPTION_KEY", "test-medulla-encryption-key")
    prebuilt = _prebuilt_test_binary()
    command = (
        [prebuilt, "--socket-path", socket_path]
        if prebuilt is not None
        else [
            "cargo", "run", "--features", "full", "--bin", "medulla-server", "--",
            "--socket-path", socket_path,
        ]  # fmt: skip
    )
    if os.path.exists(socket_path):
        os.remove(socket_path)
    rust_dir = str(Path(__file__).resolve().parents[2])
    process = spawn_server(
        command, cwd=rust_dir, env={**os.environ, **env}, log_path=log_path
    )
    try:
        wait_for_server(process, socket_path, name="share engine", log_path=log_path)
        yield socket_path
    finally:
        stop_server(process, name="share engine", log_path=log_path)


@pytest.mark.spec("MEDULLA-IDENTITY-R023.13.6")
def test_org_share_revocation_persists_across_restart(tmp_path):
    """Write a node, share it to the tenant, revoke it, stop the engine with SIGTERM,
    restart it on the same data directory: a second principal does not see the node."""
    node_id = "share-restart-1"
    with _engine(tmp_path) as socket_path:
        bootstrap = SyncMedullaClient.connect(
            socket_path=socket_path,
            auth_secret=AUTH_SECRET,
            verified_context=bootstrap_context(),
        )
        try:
            bootstrap.consensus.bootstrap_system_identity(
                agent_id=TEST_AGENT_ID,
                signer_id=TEST_AGENT_ID,
                signer_key=TEST_SIGNER_KEY,
            )
        finally:
            bootstrap.close()
        register_commons_agents(OWNER, OTHER, socket_path=socket_path, auth_secret=AUTH_SECRET)
        owner = _connect(socket_path, OWNER)
        other = _connect(socket_path, OTHER)
        try:
            owner.nodes.add(node_id, _note(OWNER))
            assert _note_ids(other) == []
            owner.rbac.share_grant(node_id, "tenant")
            assert _note_ids(other) == [node_id]
            owner.rbac.share_revoke(node_id)
            assert _note_ids(other) == []
        finally:
            owner.close()
            other.close()

    with _engine(tmp_path) as socket_path:
        other = _connect(socket_path, OTHER)
        owner = _connect(socket_path, OWNER)
        try:
            assert _note_ids(other) == []
            assert _note_ids(owner) == [node_id]
        finally:
            other.close()
            owner.close()
