"""Served pre-login SSO state and SAML assertion replay.

Specs: MEDULLA-IDENTITY-R750 and MEDULLA-IDENTITY-R751.

Both ride the served ``Identity`` method as ops of the ``token`` family, submitted by
the broker (``identity:authenticate``). The test starts its own engine and holds the
login state of one OIDC sign-in before any session exists, spends it once and reads its
nonce and PKCE verifier back, refuses the second spend, then registers one SAML
assertion id and refuses its replay. Nothing is mocked.

Run against a prebuilt full binary with ``MEDULLA_TEST_BINARY``; the module never
builds.
"""

from __future__ import annotations

import os
import secrets
import time
import uuid

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
SCOPES = [
    "identity:admin",
    "identity:read",
    "identity:authenticate",
    "security:check",
]


def _token(client: SyncMedullaClient, op: str, request: dict) -> dict:
    reply = client._send(
        "Identity",
        {"op": {"family": "token", "op": op, "request": request}},
        None,
        idempotency_key=f"login-state-{uuid.uuid4().hex}",
    )
    assert isinstance(reply, dict), reply
    return reply


@pytest.fixture(scope="module")
def engine(tmp_path_factory):
    binary = find_server_binary()
    assert binary, (
        "supply a prebuilt full MEDULLA_TEST_BINARY; no cold build is attempted"
    )
    runtime = tmp_path_factory.mktemp("login-state")
    socket_path = str(runtime / "engine.sock")
    persist = runtime / "persist"
    persist.mkdir()
    log_path = runtime / "engine.log"
    env = strict_server_env(
        str(runtime / "security"), auth_secret=SECRET, persist_dir=str(persist)
    )
    env.update(
        MEDULLA_ENCRYPTION_KEY="login-state-test-encryption-key",
        MEDULLA_UQL_TEXT_EMBEDDER="hash",
    )
    process = spawn_server(
        [binary, "--socket-path", socket_path],
        env={**os.environ, **env},
        log_path=log_path,
    )
    try:
        wait_for_server(
            process, socket_path, name="login state engine", log_path=log_path
        )
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
            verified_context=request_context(scopes=SCOPES),
            timeout=server_timeout(),
        ) as broker:
            broker._send(
                "Identity",
                {
                    "op": {
                        "family": "config",
                        "op": "initialize",
                        "request": {
                            "mode": "local",
                            "admin_username": "login-state-root",
                            "admin_password": "correct horse battery staple",
                        },
                    }
                },
                None,
                idempotency_key=f"login-state-{uuid.uuid4().hex}",
            )
            broker._send(
                "Identity",
                {
                    "op": {
                        "family": "idp",
                        "op": "upsert",
                        "request": {
                            "idp_id": "saml-main",
                            "kind": "saml",
                            "display_name": "saml-main",
                            "enabled": True,
                            "config_json": "{}",
                            "secret_ref": None,
                            "jit_policy": "deny",
                            "email_domains": [],
                            "order": 0,
                            "rules": [],
                        },
                    }
                },
                None,
                idempotency_key=f"login-state-{uuid.uuid4().hex}",
            )
            yield broker
    finally:
        stop_server(process, name="login state engine", log_path=log_path)


@pytest.mark.spec("MEDULLA-IDENTITY-R750")
def test_login_state_is_held_before_a_session_and_spent_once(engine) -> None:
    state = secrets.token_urlsafe(48)
    nonce = secrets.token_urlsafe(24)
    verifier = secrets.token_urlsafe(48)
    _token(
        engine,
        "register_login_state",
        {"state": state, "nonce": nonce, "pkce_verifier": verifier, "ttl_ms": 600_000},
    )
    released = _token(engine, "consume_login_state", {"state": state})
    assert released["kind"] == "login_state"
    assert released["value"] == {"nonce": nonce, "pkce_verifier": verifier}

    with pytest.raises(EngineResponseError) as spent:
        _token(engine, "consume_login_state", {"state": state})
    assert "IDENTITY_TOKEN_SPENT" in str(spent.value)
    with pytest.raises(EngineResponseError) as unknown:
        _token(engine, "consume_login_state", {"state": secrets.token_urlsafe(48)})
    assert "IDENTITY_TOKEN_SPENT" in str(unknown.value)
    with pytest.raises(EngineResponseError) as weak:
        _token(
            engine,
            "register_login_state",
            {
                "state": "short",
                "nonce": nonce,
                "pkce_verifier": verifier,
                "ttl_ms": 1000,
            },
        )
    assert "IDENTITY_" in str(weak.value)


@pytest.mark.spec("MEDULLA-IDENTITY-R751")
def test_a_saml_assertion_id_registers_once(engine) -> None:
    assertion_id = f"_{uuid.uuid4().hex}"
    until = int(time.time() * 1000) + 300_000
    request = {
        "idp_id": "saml-main",
        "assertion_id": assertion_id,
        "not_on_or_after_ms": until,
    }
    first = _token(engine, "register_assertion", request)
    assert first["value"] == {"changed": True}
    with pytest.raises(EngineResponseError) as replay:
        _token(engine, "register_assertion", request)
    assert "IDENTITY_REPLAY" in str(replay.value)
    with pytest.raises(EngineResponseError) as unknown:
        _token(engine, "register_assertion", {**request, "idp_id": "saml-none"})
    assert "IDENTITY_NOT_FOUND" in str(unknown.value)
