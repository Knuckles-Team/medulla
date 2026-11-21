"""Served wire logins (MEDULLA-ENGINE-PARITY-R007.4).

Grant and revoke ride the served ``Identity`` method as the ``wire_login`` op family. The
test starts its own engine with a loopback pgwire listener and ``wire_login.cache_ttl_s`` set
to 1 second, grants a password login, logs in as it over pgwire, refuses a regrant to another
principal, revokes the login, and asserts the next login after the cache window is refused.
Nothing is mocked; Python sends authenticated RPCs and a real ``psycopg`` connection.

Run against a prebuilt full binary with ``MEDULLA_TEST_BINARY``; the module never builds.
"""

from __future__ import annotations

import hashlib
import os
import secrets
import socket
import time
import uuid

import psycopg
import pytest
from conftest import (
    TEST_AGENT_ID,
    TEST_SIGNER_KEY,
    bootstrap_context,
    find_server_binary,
    request_context,
    server_timeout,
    spawn_server,
    stop_server,
    strict_server_env,
    wait_for_server,
)

from medulla.client import EngineResponseError, SyncMedullaClient

SECRET = secrets.token_hex(32)
ADMIN_SCOPES = [
    "identity:admin",
    "identity:read",
    "identity:authenticate",
    "security:check",
]
# The SCRAM iteration count the pgwire listener checks a proof against.
SCRAM_ITERATIONS = 4096
CACHE_TTL_S = 1


def _free_port() -> int:
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        return probe.getsockname()[1]


def _password_verifier(password: str) -> str:
    """The stored form of a password login: ``<salt hex>:<salted password hex>``."""
    salt = os.urandom(16)
    salted = hashlib.pbkdf2_hmac("sha256", password.encode(), salt, SCRAM_ITERATIONS)
    return f"{salt.hex()}:{salted.hex()}"


def _identity(client: SyncMedullaClient, op: dict) -> object:
    return client._send(
        "Identity", {"op": op}, None, idempotency_key=f"wire-login-{uuid.uuid4().hex}"
    )


def _wire_login(client: SyncMedullaClient, op: str, request: dict) -> object:
    return _identity(client, {"family": "wire_login", "op": op, "request": request})


def _value(reply: object) -> dict:
    """The payload of an ``IdentityReply`` (``{"kind": ..., "value": ...}``)."""
    assert isinstance(reply, dict), reply
    return reply.get("value", reply)


@pytest.fixture(scope="module")
def engine(tmp_path_factory):
    """One engine with a loopback pgwire listener and a 1 second wire-login cache."""
    binary = find_server_binary()
    assert binary, (
        "supply a prebuilt full MEDULLA_TEST_BINARY; no cold build is attempted"
    )
    runtime = tmp_path_factory.mktemp("wire-login")
    socket_path = str(runtime / "engine.sock")
    persist = runtime / "persist"
    persist.mkdir()
    log_path = runtime / "engine.log"
    pg_port = _free_port()
    env = strict_server_env(
        str(runtime / "security"), auth_secret=SECRET, persist_dir=str(persist)
    )
    env.update(
        MEDULLA_ENCRYPTION_KEY="wire-login-test-encryption-key",
        MEDULLA_PGWIRE_ADDR=f"127.0.0.1:{pg_port}",
        MEDULLA_WIRE_LOGIN_CACHE_TTL_S=str(CACHE_TTL_S),
        MEDULLA_UQL_TEXT_EMBEDDER="hash",
    )
    process = spawn_server(
        [binary, "--socket-path", socket_path], env={**os.environ, **env}, log_path=log_path
    )
    try:
        wait_for_server(process, socket_path, name="wire login engine", log_path=log_path)
        with SyncMedullaClient.connect(
            socket_path=socket_path,
            auth_secret=SECRET,
            verified_context=bootstrap_context(),
        ) as bootstrap:
            bootstrap.consensus.bootstrap_system_identity(
                agent_id=TEST_AGENT_ID,
                signer_id=TEST_AGENT_ID,
                signer_key=TEST_SIGNER_KEY,
            )
        with SyncMedullaClient.connect(
            socket_path=socket_path,
            auth_secret=SECRET,
            verified_context=request_context(scopes=ADMIN_SCOPES),
            timeout=server_timeout(),
        ) as admin:
            _identity(
                admin,
                {
                    "family": "config",
                    "op": "initialize",
                    "request": {
                        "mode": "local",
                        "admin_username": "wire-login-root",
                        "admin_password": "correct horse battery staple",
                    },
                },
            )
            yield admin, pg_port
    finally:
        stop_server(process, name="wire login engine", log_path=log_path)


def _login(port: int, login: str, password: str) -> object:
    with psycopg.connect(
        host="127.0.0.1",
        port=port,
        user=login,
        password=password,
        dbname="__commons__",
        connect_timeout=10,
    ) as connection:
        return connection.execute("SELECT 1").fetchone()


@pytest.mark.spec("MEDULLA-ENGINE-PARITY-R007.4")
def test_grant_login_revoke(engine) -> None:
    admin, port = engine
    tag = uuid.uuid4().hex[:8]
    login = f"bi-{tag}"
    password = secrets.token_hex(16)
    grant = {
        "login": login,
        "kind": "Password",
        "verifier": _password_verifier(password),
        "principal": TEST_AGENT_ID,
    }

    granted = _value(_wire_login(admin, "grant", grant))
    assert granted["login"] == login
    assert granted["kind"] == "Password"
    assert granted["principal"] == TEST_AGENT_ID
    assert granted["tenant"] == request_context()["tenant"]
    assert "verifier" not in granted, "the verifier never leaves the store"

    # An identical grant replays the stored record unchanged.
    assert _value(_wire_login(admin, "grant", grant)) == granted

    assert _login(port, login, password) == (1,)

    with pytest.raises(EngineResponseError) as exists:
        _wire_login(admin, "grant", {**grant, "principal": f"{TEST_AGENT_ID}-other"})
    assert "WIRE_LOGIN_EXISTS" in str(exists.value)

    with pytest.raises(EngineResponseError) as malformed:
        _wire_login(admin, "grant", {**grant, "login": f"bad-{tag}", "verifier": "nope"})
    assert "IDENTITY_INVALID" in str(malformed.value)

    revoked = _value(_wire_login(admin, "revoke", {"login": login}))
    assert revoked["login"] == login
    # A revoke of a revoked login returns the stored record unchanged.
    assert _value(_wire_login(admin, "revoke", {"login": login})) == revoked

    time.sleep(CACHE_TTL_S + 0.5)
    with pytest.raises(psycopg.OperationalError) as refused:
        _login(port, login, password)
    assert refused.value.sqlstate in (None, "28000", "28P01"), refused.value
    assert "INVALID_LOGIN" in str(refused.value) or "authentication failed" in str(
        refused.value
    )

    with pytest.raises(EngineResponseError) as unknown:
        _wire_login(admin, "revoke", {"login": f"never-{tag}"})
    assert "WIRE_LOGIN_UNKNOWN" in str(unknown.value)
