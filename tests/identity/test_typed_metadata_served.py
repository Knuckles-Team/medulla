"""Served typed principal metadata (MEDULLA-IDENTITY-R023.8 to R023.12).

Each case runs against its own authenticated engine on a durable store, because
the proofs restart that engine. A principal writes `Preference`, `ClientMetadata`
and `InteractionNetwork` records through the served node write; the engine
validates them, stamps the owner from the verified principal and the version and
provenance keys, and keeps them across a restart.
"""

from __future__ import annotations

import os
import shutil
import tempfile
from collections.abc import Iterator
from contextlib import contextmanager
from pathlib import Path

import pytest
from conftest import (
    TEST_AGENT_ID,
    TEST_SIGNER_KEY,
    TEST_TENANT,
    bootstrap_context,
    find_server_binary,
    request_context,
    spawn_server,
    stop_server,
    strict_server_env,
    wait_for_server,
)

from medulla.client import EngineResponseError, SyncMedullaClient
from medulla.generated.models import (
    CreateUserRequest,
    IdentityOpUserCreate,
    MethodIdentityParams,
    UserKind,
)
from medulla.generated.security import send_identity

_SERVER_BIN = find_server_binary()
SECRET = "metadata-test-secret"  # sanitizer:ignore - test-only value

OWNER = "usr:alice"
PEER = "usr-peer"
OTHER_MEMBER = "usr:bob"
OTHER_TENANT_MEMBER = "usr:carol"
OTHER_TENANT = "tenant:elsewhere"
DIGEST = "ab" * 32

pytestmark = [
    pytest.mark.no_engine,  # each case owns its real engine and restarts it
    pytest.mark.skipif(_SERVER_BIN is None, reason="medulla-server not built"),
]


class Engine:
    """One durable engine that can be stopped and started again on its store."""

    def __init__(self, root: str) -> None:
        self.root = root
        self.socket_path = os.path.join(root, "engine.sock")
        self.persist = os.path.join(root, "persist")
        os.makedirs(self.persist, exist_ok=True)
        self.process = None
        self.log = Path(root) / "engine.log"
        self.bootstrapped = False

    def start(self) -> None:
        assert _SERVER_BIN is not None
        env = {
            **os.environ,
            **strict_server_env(
                os.path.join(self.root, "security"),
                auth_secret=SECRET,
                persist_dir=self.persist,
            ),
        }
        env.setdefault("MEDULLA_ENCRYPTION_KEY", "test-medulla-encryption-key")
        if os.path.exists(self.socket_path):
            os.remove(self.socket_path)
        self.process = spawn_server(
            [_SERVER_BIN, "--socket-path", self.socket_path],
            env=env,
            log_path=self.log,
        )
        wait_for_server(
            self.process, self.socket_path, name="metadata", log_path=self.log
        )
        if not self.bootstrapped:
            boot = SyncMedullaClient.connect(
                socket_path=self.socket_path,
                auth_secret=SECRET,
                verified_context=bootstrap_context(),
            )
            try:
                boot.consensus.bootstrap_system_identity(
                    agent_id=TEST_AGENT_ID,
                    signer_id=TEST_AGENT_ID,
                    signer_key=TEST_SIGNER_KEY,
                )
            finally:
                boot.close()
            self.bootstrapped = True

    def stop(self) -> None:
        if self.process is not None:
            stop_server(self.process, name="metadata", log_path=self.log)
            self.process = None

    def restart(self) -> None:
        self.stop()
        self.start()

    def connect(
        self, principal: str, tenant: str = TEST_TENANT, scopes: list[str] | None = None
    ) -> SyncMedullaClient:
        context = request_context(principal=principal, scopes=scopes)
        context["tenant"] = tenant
        return SyncMedullaClient.connect(
            socket_path=self.socket_path, auth_secret=SECRET, verified_context=context
        )


@contextmanager
def engine() -> Iterator[Engine]:
    root = tempfile.mkdtemp(prefix="medulla-meta-")
    served = Engine(root)
    served.start()
    try:
        yield served
    finally:
        served.stop()
        shutil.rmtree(root, ignore_errors=True)


def _register_peer(served: Engine) -> None:
    """Create the peer principal in the identity store so it resolves."""
    admin = served.connect("usr:admin", scopes=["identity:admin"])
    try:
        op = IdentityOpUserCreate(
            family="user",
            op="create",
            request=CreateUserRequest(
                kind=UserKind.SERVICE, username="peer", principal_id=PEER
            ),
        )
        params = MethodIdentityParams(op=op).model_dump(mode="json", exclude_none=True)
        import asyncio

        asyncio.run_coroutine_threadsafe(
            send_identity(admin._client, params), admin._loop
        ).result(timeout=30)
    finally:
        admin.close()


def _preference(key: str, share: str, **extra) -> dict[str, object]:
    return {
        "type": "Preference",
        "key": key,
        "value": "dark",
        "share": share,
        **extra,
    }


def _client_metadata(**extra) -> dict[str, object]:
    return {
        "type": "ClientMetadata",
        "client_id": "cli:0001",
        "kind": "cli",
        "version": "1.0.0",
        "capabilities": ["read", "write"],
        **extra,
    }


def _network(**extra) -> dict[str, object]:
    return {
        "type": "InteractionNetwork",
        "subject": OWNER,
        "peer": PEER,
        "relation": "collaborates",
        "weight_milli": 750,
        **extra,
    }


def _ids(client: SyncMedullaClient, label: str) -> set[str]:
    return {row["id"] for row in client.query.uql(f"MATCH (:{label})").get("rows", [])}


def _all_ids(client: SyncMedullaClient) -> dict[str, set[str]]:
    labels = ("Preference", "ClientMetadata", "InteractionNetwork")
    return {label: _ids(client, label) for label in labels}


def _refused(call, code: str) -> None:
    with pytest.raises((EngineResponseError, RuntimeError)) as refused:
        call()
    assert code in str(refused.value)


@pytest.mark.spec("MEDULLA-IDENTITY-R023.8")
# spec: MEDULLA-IDENTITY-R023.8
def test_typed_metadata_persists_without_file_authority():
    """A restart keeps all three variants and their revision chain."""
    with engine() as served:
        _register_peer(served)
        owner = served.connect(OWNER)
        try:
            owner.nodes.add("pref-1", _preference("theme", "private"))
            owner.nodes.add(
                "pref-2",
                _preference("theme", "private", supersedes="pref-1", parent_revision=1),
            )
            owner.nodes.add("client-1", _client_metadata())
            owner.nodes.add("net-1", _network())
        finally:
            owner.close()

        served.restart()

        owner = served.connect(OWNER)
        try:
            seen = _all_ids(owner)
            assert seen["Preference"] == {"pref-1", "pref-2"}
            assert seen["ClientMetadata"] == {"client-1"}
            assert seen["InteractionNetwork"] == {"net-1"}
            second = owner.nodes.properties("pref-2")
            assert second["owner"] == OWNER
            assert second["revision"] == 2
            assert second["supersedes"] == "pref-1"
        finally:
            owner.close()


@pytest.mark.spec("MEDULLA-IDENTITY-R023.9")
# spec: MEDULLA-IDENTITY-R023.9
def test_all_metadata_write_routes_reach_admission():
    """Node writes and statement routes are each admitted or refused by `admit`."""
    with engine() as served:
        owner = served.connect(OWNER)
        try:
            # AddNode: an untyped body is refused by admission.
            _refused(
                lambda: owner.nodes.add("bad-1", {"type": "Preference"}),
                "METADATA_UNTYPED",
            )
            # Statement routes cannot write the label either.
            _refused(
                lambda: owner.query.sql(
                    "INSERT INTO nodes (id, type) VALUES ('bad-2', 'Preference')"
                ),
                "METADATA_UNTYPED",
            )
            # A typed AddNode is admitted and stamped with the verified owner.
            owner.nodes.add(
                "ok-1", _preference("theme", "private", owner="usr:mallory")
            )
            assert owner.nodes.properties("ok-1")["owner"] == OWNER
        finally:
            owner.close()


@pytest.mark.spec("MEDULLA-IDENTITY-R023.10")
# spec: MEDULLA-IDENTITY-R023.10
def test_invalid_metadata_has_no_durable_effect():
    """An independent read shows unchanged state after each refusal."""
    with engine() as served:
        _register_peer(served)
        owner = served.connect(OWNER)
        reader = served.connect(OWNER)
        try:
            owner.nodes.add("seed", _preference("seed", "private"))
            before = _all_ids(reader)
            cases = [
                (
                    lambda: owner.nodes.add("r1", {"type": "Preference"}),
                    "METADATA_UNTYPED",
                ),
                (
                    lambda: owner.nodes.add("r2", _preference("k" * 65, "private")),
                    "METADATA_SCHEMA_INVALID",
                ),
                (
                    lambda: owner.nodes.add("r3", _network(peer="usr:nobody")),
                    "METADATA_PRINCIPAL_UNKNOWN",
                ),
                (
                    lambda: owner.nodes.add("r4", _network(subject=OTHER_MEMBER)),
                    "ACCESS_DENIED",
                ),
            ]
            for call, code in cases:
                _refused(call, code)
                assert _all_ids(reader) == before
        finally:
            owner.close()
            reader.close()


@pytest.mark.spec("MEDULLA-IDENTITY-R023.11")
# spec: MEDULLA-IDENTITY-R023.11
def test_metadata_import_provenance_survives_restart():
    """A replay after restart creates no second node and keeps the provenance keys."""
    with engine() as served:
        owner = served.connect(OWNER)
        try:
            props = _client_metadata(source_digest=DIGEST, mapping_version="m1")
            owner.nodes.add("imp-1", props)
            stored = owner.nodes.properties("imp-1")
            assert stored["source_digest"] == DIGEST
            assert stored["mapping_version"] == "m1"
            assert stored["actor"] == OWNER
            assert stored["schema_digest"]
            assert stored["parent_revision"] == 0
            # A digest that is not 64 lowercase hex is refused.
            _refused(
                lambda: owner.nodes.add(
                    "imp-2", _client_metadata(source_digest="XYZ", mapping_version="m1")
                ),
                "METADATA_SCHEMA_INVALID",
            )
        finally:
            owner.close()

        served.restart()

        owner = served.connect(OWNER)
        try:
            owner.nodes.add(
                "imp-1", _client_metadata(source_digest=DIGEST, mapping_version="m1")
            )
            assert _ids(owner, "ClientMetadata") == {"imp-1"}
            assert owner.nodes.properties("imp-1")["source_digest"] == DIGEST
        finally:
            owner.close()


@pytest.mark.spec("MEDULLA-IDENTITY-R023.12")
# spec: MEDULLA-IDENTITY-R023.12
def test_typed_metadata_served_journey():
    """Write three variants, restart, read as the owner and as other principals."""
    with engine() as served:
        _register_peer(served)
        owner = served.connect(OWNER)
        try:
            owner.nodes.add("pref-private", _preference("a", "private"))
            owner.nodes.add("pref-shared", _preference("b", "tenant"))
            owner.nodes.add("client-1", _client_metadata())
            owner.nodes.add("net-1", _network())
            # A denied write and a malformed write leave an independent read unchanged.
            baseline = _all_ids(owner)
            _refused(
                lambda: owner.nodes.add("deny", _network(subject=OTHER_MEMBER)),
                "ACCESS_DENIED",
            )
            _refused(
                lambda: owner.nodes.add("bad", {"type": "ClientMetadata"}),
                "METADATA_UNTYPED",
            )
            assert _all_ids(owner) == baseline
        finally:
            owner.close()

        served.restart()

        owner = served.connect(OWNER)
        member = served.connect(OTHER_MEMBER)
        outsider = served.connect(OTHER_TENANT_MEMBER, tenant=OTHER_TENANT)
        try:
            # The owner sees all three variants.
            seen = _all_ids(owner)
            assert seen["Preference"] == {"pref-private", "pref-shared"}
            assert seen["ClientMetadata"] == {"client-1"}
            assert seen["InteractionNetwork"] == {"net-1"}
            # Another principal of the tenant sees only the tenant-shared preference.
            assert _all_ids(member) == {
                "Preference": {"pref-shared"},
                "ClientMetadata": set(),
                "InteractionNetwork": set(),
            }
            # A principal of another tenant sees none of it.
            assert all(not ids for ids in _all_ids(outsider).values())
        finally:
            owner.close()
            member.close()
            outsider.close()
