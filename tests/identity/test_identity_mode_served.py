"""Served identity-mode transition authority (MEDULLA-IDENTITY-R003.7).

The served ``Identity`` method's mode transition needs the ``identity:admin`` scope. A
principal that holds only a read scope is refused, and the refusal leaves the engine's
identity configuration unchanged.
"""

import asyncio
import contextlib
import os
import secrets
from pathlib import Path

import pytest

from medulla.client import EngineResponseError, SyncMedullaClient
from medulla.generated.models import (
    AuthenticateRequest,
    AuthMode,
    ExternalLogin,
    IdentityOpConfigGet,
    IdentityOpConfigInitialize,
    IdentityOpConfigTransition,
    IdentityOpCredentialAuthenticate,
    IdentityOpCredentialExternalLogin,
    IdentityOpIdpLink,
    IdentityOpIdpUpsert,
    IdentityOpSessionResolve,
    IdpConfig,
    IdpKind,
    InitializeRequest,
    JitPolicy,
    LinkRequest,
    MethodIdentityParams,
    ModeTransition,
    SessionTouch,
)
from medulla.generated.security import send_identity

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
        wait_for_server,
    )

AUTH_SECRET = "test-medulla-secret"
NODE_OWNER = "usr:mode-writer"
ADMIN_PASSWORD = "mode-admin-passphrase-01"


def _send(client, op):
    params = MethodIdentityParams(op=op).model_dump(mode="json", exclude_none=True)
    return asyncio.run_coroutine_threadsafe(
        send_identity(client._client, params), client._loop
    ).result(timeout=30)


def _connect(scopes):
    return SyncMedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(principal="mode-reader", scopes=scopes),
    )


@pytest.mark.spec("MEDULLA-IDENTITY-R003.7")
def test_non_admin_cannot_transition():
    reader = _connect(["identity:read"])
    transition = IdentityOpConfigTransition(
        family="config",
        op="transition",
        request=ModeTransition(
            expected_epoch=0, issuer_kid="mode-test-kid", to=AuthMode.EXTERNAL
        ),
    )
    with pytest.raises((EngineResponseError, RuntimeError)):
        _send(reader, transition)

    # The refusal changed nothing: the identity configuration reads as it did.
    get = IdentityOpConfigGet(family="config", op="get")
    before = _send(reader, get)
    with pytest.raises((EngineResponseError, RuntimeError)):
        _send(reader, transition)
    assert _send(reader, get) == before


@contextlib.contextmanager
def _engine(root: Path):
    """A dedicated engine on a fresh data directory whose identity store is uninitialized
    (the state ``ConfigOp::Initialize`` accepts); leaving the block stops it."""
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
        wait_for_server(process, socket_path, name="mode engine", log_path=log_path)
        yield socket_path
    finally:
        stop_server(process, name="mode engine", log_path=log_path)


def _dump(value):
    dump = getattr(value, "model_dump", None)
    if dump is not None:
        return dump(mode="json")
    payload = getattr(value, "payload", value)
    dump = getattr(payload, "model_dump", None)
    return dump(mode="json") if dump is not None else payload


def _field(value, key):
    """The first value stored under `key` anywhere in a (possibly wrapped) reply."""
    value = _dump(value)
    if isinstance(value, dict):
        if key in value:
            return value[key]
        for inner in value.values():
            found = _field(inner, key)
            if found is not None:
                return found
    elif isinstance(value, list):
        for inner in value:
            found = _field(inner, key)
            if found is not None:
                return found
    return None


def _transition(admin, expected_epoch, to, kid):
    return _send(
        admin,
        IdentityOpConfigTransition(
            family="config",
            op="transition",
            request=ModeTransition(expected_epoch=expected_epoch, to=to, issuer_kid=kid),
        ),
    )


def _resolve(admin, token):
    return _send(
        admin,
        IdentityOpSessionResolve(
            family="session",
            op="resolve",
            request=SessionTouch(refresh=False, session_token=token),
        ),
    )


@pytest.mark.spec("MEDULLA-IDENTITY-R003.8")
def test_local_external_round_trip(tmp_path):
    """The local to external to local mode round trip over the served wire: an unlinked
    transition refuses, a linked one revokes every older session, a new session under
    the new mode resolves, and the epoch guards both stale and forbidden transitions."""
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
        register_commons_agents(NODE_OWNER, socket_path=socket_path, auth_secret=AUTH_SECRET)
        admin = SyncMedullaClient.connect(
            socket_path=socket_path,
            auth_secret=AUTH_SECRET,
            verified_context=request_context(
                agent_id="mode-admin",
                scopes=["identity:admin", "identity:authenticate", "identity:read"],
            ),
        )
        writer = SyncMedullaClient.connect(
            socket_path=socket_path,
            auth_secret=AUTH_SECRET,
            verified_context=request_context(agent_id=NODE_OWNER),
        )
        try:
            # (1) the local administrator.
            initialized = _send(
                admin,
                IdentityOpConfigInitialize(
                    family="config",
                    op="initialize",
                    request=InitializeRequest(
                        mode=AuthMode.LOCAL,
                        admin_username="admin",
                        admin_password=ADMIN_PASSWORD,
                    ),
                ),
            )
            assert _field(initialized, "epoch") == 1

            # (2) the older session.
            old_token = secrets.token_hex(32)
            authenticated = _send(
                admin,
                IdentityOpCredentialAuthenticate(
                    family="credential",
                    op="authenticate",
                    request=AuthenticateRequest(
                        username="admin",
                        password=ADMIN_PASSWORD,
                        session_token=old_token,
                    ),
                ),
            )
            assert _field(authenticated, "ok") is True
            _resolve(admin, old_token)

            # (3) a node, then the transition refuses before any link.
            writer.nodes.add("mode-node-1", {"node_type": "Note", "_owner_id": NODE_OWNER})
            node_before = writer.nodes.properties("mode-node-1")
            with pytest.raises(EngineResponseError, match="IDENTITY_PRECONDITION_FAILED"):
                _transition(admin, 1, AuthMode.EXTERNAL, "mode-kid-1")
            assert _field(_send(admin, IdentityOpConfigGet(family="config", op="get")), "epoch") == 1

            # (4) link the administrator to an external identity.
            _send(
                admin,
                IdentityOpIdpUpsert(
                    family="idp",
                    op="upsert",
                    request=IdpConfig(
                        idp_id="mode-idp",
                        kind=IdpKind.OIDC,
                        display_name="mode idp",
                        enabled=True,
                        config_json="{}",
                        jit_policy=JitPolicy.DENY,
                    ),
                ),
            )
            _send(
                admin,
                IdentityOpIdpLink(
                    family="idp",
                    op="link",
                    request=LinkRequest(
                        idp_id="mode-idp",
                        subject="mode-admin-subject",
                        principal_id="usr:bootstrap",
                    ),
                ),
            )

            # (5) the transition now succeeds.
            assert _field(_transition(admin, 1, AuthMode.EXTERNAL, "mode-kid-1"), "epoch") == 2

            # (6) the older session no longer resolves.
            with pytest.raises(EngineResponseError):
                _resolve(admin, old_token)

            # (7) a new session under the new mode resolves for the linked administrator.
            new_token = secrets.token_hex(32)
            login = _send(
                admin,
                IdentityOpCredentialExternalLogin(
                    family="credential",
                    op="external_login",
                    request=ExternalLogin(
                        idp_id="mode-idp",
                        subject="mode-admin-subject",
                        session_token=new_token,
                    ),
                ),
            )
            assert _field(login, "ok") is True
            assert "usr:bootstrap" in str(_dump(_resolve(admin, new_token)))
            assert writer.nodes.properties("mode-node-1") == node_before

            # (8) back to local: the epoch advanced twice.
            assert _field(_transition(admin, 2, AuthMode.LOCAL, "mode-kid-2"), "epoch") == 3

            # (9) a call with the stale epoch refuses.
            with pytest.raises(EngineResponseError, match="IDENTITY_EPOCH_CONFLICT"):
                _transition(admin, 2, AuthMode.EXTERNAL, "mode-kid-3")

            # (10) mode none is forbidden.
            with pytest.raises(EngineResponseError, match="MODE_TRANSITION_FORBIDDEN"):
                _transition(admin, 3, AuthMode.NONE, "mode-kid-4")
        finally:
            admin.close()
            writer.close()
