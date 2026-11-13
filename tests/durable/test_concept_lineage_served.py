"""Served proof for MEDULLA-DURABLE-KERNEL-R094.7 and R094.7.1.

Concept ids and their `superseded_by` edges are served by `ConceptLineage`
over the real native engine, reached through the signed client: claimed ids
and edges survive an engine restart, a path claimed by another repository is
refused, and an edge that is a self edge, names an unknown id or closes a
cycle is refused by its registered code.
"""

from __future__ import annotations

import asyncio
import contextlib
import secrets

import msgpack
import pytest
from conftest import (
    TEST_AGENT_ID,
    TEST_SIGNER_KEY,
    TEST_TENANT,
    bootstrap_context,
    find_server_binary,
    request_context,
    server_timeout,
    spawn_server,
    stop_server,
    strict_server_env,
    wait_for_server,
)

from medulla.client import MedullaClient, SyncMedullaClient

pytestmark = [
    pytest.mark.no_engine,  # Each case owns its real isolated process.
]

SECRET = secrets.token_hex(32)
PLACEHOLDER = "bound-by-engine"


def _raw(value):
    """A raw method result travels as the MessagePack of its value."""
    if isinstance(value, (bytes, bytearray)):
        return msgpack.unpackb(value, raw=False)
    return value


def _context(key):
    """A request-body context shell; the engine re-binds every field of it."""
    return {
        "request_id": 1,
        "principal": PLACEHOLDER,
        "caller_principal": PLACEHOLDER,
        "attempt_nonce": "00" * 32,
        "tenant_id": TEST_TENANT,
        "actor_scope": PLACEHOLDER,
        "purpose_id": PLACEHOLDER,
        "policy_revision": PLACEHOLDER,
        "policy_digest": "sha256:" + "00" * 32,
        "policy_decision_id": PLACEHOLDER,
        "idempotency_key": key,
        "created_at_ms": 1,
    }


@contextlib.contextmanager
def _engine(runtime, *, first_start):
    """One engine over `runtime`'s durable state; a second start reuses it."""
    binary = find_server_binary()
    assert binary, (
        "supply a prebuilt full MEDULLA_TEST_BINARY; no cold build is attempted"
    )
    socket_path = str(runtime / "engine.sock")
    persist = runtime / "persist"
    persist.mkdir(exist_ok=True)
    log_path = runtime / f"engine-{'first' if first_start else 'second'}.log"
    env = strict_server_env(
        str(runtime / "security"), auth_secret=SECRET, persist_dir=str(persist)
    )
    env["MEDULLA_ENCRYPTION_KEY"] = "concept-lineage-test-encryption-key"
    process = spawn_server(
        [binary, "--socket-path", socket_path], env=env, log_path=log_path
    )
    try:
        wait_for_server(
            process, socket_path, name="concept lineage engine", log_path=log_path
        )
        if first_start:
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
        yield socket_path
    finally:
        stop_server(process, name="concept lineage engine", log_path=log_path)


async def _connect(socket_path):
    return await MedullaClient.connect(
        socket_path=socket_path,
        auth_secret=SECRET,
        verified_context=request_context(),
        timeout=server_timeout(),
    )


async def _op(client, op, key=None):
    return _raw(
        await client._send("ConceptLineage", {"op": op}, None, idempotency_key=key)
    )


async def _claim(client, key, concept_id, repository):
    return await _op(
        client,
        {
            "op": "claim",
            "context": _context(key),
            "concept_id": concept_id,
            "repository": repository,
        },
        key,
    )


async def _edge(client, key, superseded, successor):
    return await _op(
        client,
        {
            "op": "add_edge",
            "context": _context(key),
            "superseded": superseded,
            "successor": successor,
            "repository": "medulla",
        },
        key,
    )


async def _read(client, concept_id):
    return await _op(
        client, {"op": "read", "tenant_id": TEST_TENANT, "concept_id": concept_id}
    )


async def _refused(call, code):
    with pytest.raises(RuntimeError) as refusal:
        await call
    assert code in str(refusal.value), refusal.value


# spec: MEDULLA-DURABLE-KERNEL-R094.7.1
@pytest.mark.spec("MEDULLA-DURABLE-KERNEL-R094.7.1")
def test_known_ids_and_edges_survive_restart(tmp_path):
    """Verifies MEDULLA-DURABLE-KERNEL-R094.7.1.

    Claims two ids, adds an edge, restarts the server and reads both back.
    """
    older, newer = "MEDULLA-KG.memory", "MEDULLA-KG.recall"

    async def first(socket_path):
        client = await _connect(socket_path)
        try:
            await _claim(client, "claim-older", older, "medulla")
            await _claim(client, "claim-newer", newer, "medulla")
            await _edge(client, "edge-1", older, newer)
            await _refused(
                _claim(client, "claim-collision", older, "meristem"),
                "CONCEPT_ID_PATH_COLLISION",
            )
        finally:
            await client.close()

    async def second(socket_path):
        client = await _connect(socket_path)
        try:
            seen = await _read(client, older)
            assert seen["concept"]["concept_id"] == older
            assert seen["concept"]["repository"] == "medulla"
            assert [edge["successor"] for edge in seen["successors"]] == [newer]
            assert (await _read(client, newer))["concept"]["concept_id"] == newer
            await _refused(
                _claim(client, "claim-collision-2", older, "meristem"),
                "CONCEPT_ID_PATH_COLLISION",
            )
        finally:
            await client.close()

    with _engine(tmp_path, first_start=True) as socket_path:
        asyncio.run(first(socket_path))
    with _engine(tmp_path, first_start=False) as socket_path:
        asyncio.run(second(socket_path))


# spec: MEDULLA-DURABLE-KERNEL-R094.7
@pytest.mark.spec("MEDULLA-DURABLE-KERNEL-R094.7")
def test_lineage_cycle_is_refused(tmp_path):
    """Verifies MEDULLA-DURABLE-KERNEL-R094.7.

    Adds edges through the served method and asserts the three codes.
    """
    ids = ["MEDULLA-KG.one", "MEDULLA-KG.two", "MEDULLA-KG.three"]

    async def run(socket_path):
        client = await _connect(socket_path)
        try:
            for number, concept_id in enumerate(ids):
                await _claim(client, f"claim-{number}", concept_id, "medulla")
            await _edge(client, "edge-12", ids[0], ids[1])
            await _edge(client, "edge-23", ids[1], ids[2])
            await _refused(
                _edge(client, "edge-31", ids[2], ids[0]), "CONCEPT_LINEAGE_CYCLE"
            )
            await _refused(
                _edge(client, "edge-11", ids[0], ids[0]),
                "CONCEPT_LINEAGE_SELF_EDGE",
            )
            await _refused(
                _edge(client, "edge-1x", ids[0], "MEDULLA-KG.ghost"),
                "CONCEPT_LINEAGE_UNKNOWN_ID",
            )
            assert (await _read(client, ids[2]))["successors"] == []
        finally:
            await client.close()

    with _engine(tmp_path, first_start=True) as socket_path:
        asyncio.run(run(socket_path))
