"""Served-test kit for the commons embedding generation: the connection, the
generation the first served cycle establishes, the query vector in its space and
a published feature schema (MEDULLA-DECISION-ENGINE-R172.12.6, R172.11)."""

from __future__ import annotations

import base64
import contextlib
import os
import secrets
import struct
import time
import uuid
from pathlib import Path
from typing import Any

import msgpack
from _decision_resident_fixture import publication, schema_body
from _harness_envelope import draft_envelope, op
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

from medulla import decision_client as dc
from medulla.client import EngineResponseError, MedullaClient, SyncMedullaClient

COMMONS = "__commons__"
DIM = 384


def connect(
    graph: str | None = None,
    *,
    engine: Engine | None = None,
    context: dict[str, Any] | None = None,
):
    """An authenticated client of the shared session engine, or of ``engine``,
    acting under ``context`` (the default test principal when ``None``)."""
    verified = context if context is not None else request_context()
    if engine is None:
        return MedullaClient.connect(
            socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
            verified_context=verified,
            graph_name=graph,
        )
    return MedullaClient.connect(
        socket_path=engine.socket,
        auth_secret=engine.secret,
        verified_context=verified,
        graph_name=graph,
        timeout=server_timeout(),
    )


class Engine:
    """Where a restartable engine listens and the secret its clients sign with."""

    def __init__(self, socket: str, secret: str) -> None:
        self.socket = socket
        self.secret = secret


@contextlib.contextmanager
def restartable_engine(tmp_path: Path, run: str, *, bootstrap: bool):
    """One native engine over ``tmp_path``'s durable store.

    A second call with the same ``tmp_path`` reopens the store the first one
    left behind, which is how a served test restarts the engine. Only the
    first call bootstraps the system identity.
    """
    binary = find_server_binary()
    assert binary, (
        "supply a prebuilt full MEDULLA_TEST_BINARY; no cold build is attempted"
    )
    secret_file = tmp_path / "auth-secret"
    if not secret_file.exists():
        secret_file.write_text(secrets.token_hex(32))
    secret = secret_file.read_text()
    socket = str(tmp_path / "engine.sock")
    Path(socket).unlink(missing_ok=True)
    persist = tmp_path / "persist"
    persist.mkdir(exist_ok=True)
    env = strict_server_env(
        str(tmp_path / "security"), auth_secret=secret, persist_dir=str(persist)
    )
    env.update(HOME=str(tmp_path), MEDULLA_ENCRYPTION_KEY="designation-test-key")
    log = tmp_path / f"{run}.log"
    process = spawn_server(
        [str(Path(binary).resolve()), "--socket-path", socket], env=env, log_path=log
    )
    try:
        wait_for_server(process, socket, name="designation engine", log_path=log)
        if bootstrap:
            with SyncMedullaClient.connect(
                socket_path=socket,
                auth_secret=secret,
                verified_context=bootstrap_context(),
            ) as client:
                client.consensus.bootstrap_system_identity(
                    agent_id=TEST_AGENT_ID,
                    signer_id=TEST_AGENT_ID,
                    signer_key=TEST_SIGNER_KEY,
                )
        yield Engine(socket, secret)
    finally:
        stop_server(process, name="designation engine", log_path=log)


def vector(version: str) -> dict[str, Any]:
    raw = struct.pack(f"<{DIM}f", *([1.0] + [0.0] * (DIM - 1)))
    return {
        "embedding_version": version,
        "dim": DIM,
        "f32le_b64": base64.b64encode(raw).decode(),
    }


async def commons_space(
    decision: dc.DecisionClient, model: dict[str, Any], tenant: str = TEST_TENANT
):
    return await decision.log(
        {
            "op": "generation_space",
            "tenant_id": tenant,
            "logical": COMMONS,
            "model": model,
        }
    )


async def establish_commons_generation(
    client: MedullaClient, tenant: str = TEST_TENANT
) -> dict[str, Any]:
    """The commons generation, established through the served cycle when absent."""
    decision = dc.DecisionClient(client=client)
    model = {"id": "pack-designation-model", "revision": "1", "dimension": DIM}
    try:
        await client.tenants.create(COMMONS)
    except EngineResponseError:
        pass  # already exists
    read = await commons_space(decision, model, tenant)
    if read["active"] is None:
        cell = f"cell-{uuid.uuid4().hex[:8]}"
        await client.capacity_leases.update_cell(
            {
                "schema_version": "1",
                "cell": {
                    "cell_id": cell,
                    "resource_class": "llm_embedding",
                    "capacity": 8,
                    "reserved_floor": 0,
                    "epoch": 1,
                    "policy_digest": "sha256:" + "d" * 64,
                    "updated_at_ms": int(time.time() * 1000),
                },
                "expected_epoch": None,
                "now_ms": int(time.time() * 1000),
            }
        )
        shadow = f"commons-gen-{uuid.uuid4().hex[:8]}"
        await client.tenants.create(shadow)
        cycle = dc.learn_op(
            tenant,
            "run_generation_cycle",
            request={
                "model_ref": model,
                "evaluation": {
                    "logical": COMMONS,
                    "active_graph": COMMONS,
                    "active_space": "none",
                    "shadow_graph": shadow,
                    "shadow_space": read["model_space_digest"],
                    "items": [],
                    "top_k": 5,
                    "min_eval_items": 5,
                    "max_score_psi_q16": 64 << 16,
                },
                "capacity_cell": cell,
                "max_batches": None,
            },
        )
        job = await decision.log(cycle)
        assert job["recorded"] == "cycle", job
        assert job["cycle"]["state"] == "activated", job
        read = await commons_space(decision, model, tenant)
        assert read["active"]["graph"] == shadow, read
        assert read["active"]["space_digest"] == read["model_space_digest"], read
    assert read["active"]["dimensions"] == DIM, read
    return read["active"]


async def publish_schema(client: MedullaClient) -> dict[str, Any]:
    component_id = "pack-designation-schema-" + uuid.uuid4().hex
    body = schema_body()
    result = await client._send(
        "AgentComponent", publication(component_id, "feature_schema", body)
    )
    return {
        "component_id": component_id,
        "kind": "feature_schema",
        "definition_digest": result["component"]["definition_digest"],
    }


async def write_docs(client: MedullaClient, graph: str, count: int, key: str) -> None:
    """``count`` unembedded ``Doc`` nodes in ``graph``, through the served
    change-envelope write (the generation cycle embeds them)."""
    operations = [
        op(
            ordinal,
            "AddNode",
            {
                "node_id": f"doc-{ordinal}",
                "properties_msgpack": msgpack.packb(
                    {"type": "Doc", "text": f"document number {ordinal} of {key}"}
                ),
            },
        )
        for ordinal in range(count)
    ]
    envelope = draft_envelope(graph, key, key, 1, operations)
    results = await client.changes.apply_batch([envelope])
    assert [result["status"] for result in results] == ["applied"], results
