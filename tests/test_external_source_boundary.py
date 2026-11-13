"""Outside databases are sources only (MEDULLA-ENGINE-PARITY-R019.2).

An engine record class is never placed in, or written back to, an outside source:
a write-back change set naming a durable engine store is refused over the wire and
leaves no row; an engine configured with a storage backend whose endpoint names an
outside database (postgres, mysql, sqlite, jdbc) refuses to start and names the
scheme.
"""

from __future__ import annotations

import json
import os
import subprocess
import uuid
from pathlib import Path

import pytest
from conftest import (
    TEST_TENANT,
    find_server_binary,
    request_context,
    strict_server_env,
)

from medulla.client import EngineResponseError, SyncMedullaClient

ZERO = "00" * 32


@pytest.fixture
def client():
    socket_path = os.environ["GRAPH_SERVICE_SOCKET"]
    c = SyncMedullaClient.connect(
        socket_path=socket_path, verified_context=request_context()
    )
    c.graph.clear()
    return c


def _change_set(change_set_id: str, target_class: str, key: str) -> dict:
    return {
        "schema_version": 1,
        "change_set_id": change_set_id,
        "change_set_digest": ZERO,
        "tenant_id": TEST_TENANT,
        "actor": "principal:placeholder",
        "purpose": "place an engine record in an outside source",
        "connector_id": "connector-1",
        "source_instance_id": "source-1",
        "entity_id": "entity-1",
        "field_scope": ["priority"],
        "base_source_version": "etag-1",
        "desired_patch": {"priority": "high"},
        "source_of_truth_rule": "source_wins_outside_scope",
        "field_provenance": {"priority": "approval:42"},
        "required_capability": "ticket:update",
        "policy_digest": "02" * 32,
        "authorization": {
            "mode": "proposal_approval",
            "authorization_ref": "approval:42",
            "decision_digest": "03" * 32,
            "input_digest": "04" * 32,
            "output_digest": "05" * 32,
            "authorized": True,
        },
        "idempotency_key": key,
        "expires_at_ms": 4_102_444_800_000,
        "reconciliation_procedure": "manual",
        "target_class": target_class,
    }


@pytest.mark.spec("MEDULLA-ENGINE-PARITY-R019.2")
def test_engine_data_never_syncs_out(client, tmp_path: Path) -> None:
    # Write-back of an engine record class refuses and writes nothing.
    for target in ("rbac.redb", "rbac"):
        change_set_id = f"cs-{uuid.uuid4().hex[:8]}"
        key = f"key-{uuid.uuid4().hex[:8]}"
        with pytest.raises(EngineResponseError) as refused:
            client.graph.write_back(
                {
                    "op": "create",
                    "change_set": _change_set(change_set_id, target, key),
                },
                idempotency_key=key,
            )
        assert refused.value.code == "EXTERNAL_PLACEMENT_FORBIDDEN"
        assert target in str(refused.value)
        stored = client.graph.write_back(
            {"op": "get", "tenant_id": TEST_TENANT, "change_set_id": change_set_id}
        )
        assert stored is None, "the refused change set left no row"

    # A storage backend whose endpoint is an outside database refuses to start.
    binary = find_server_binary()
    assert binary, (
        "supply a prebuilt full MEDULLA_TEST_BINARY; no cold build is attempted"
    )
    for scheme in ("postgres", "postgresql", "mysql", "sqlite", "jdbc"):
        run = tmp_path / scheme
        (run / "persist").mkdir(parents=True)
        env = {
            **os.environ,
            **strict_server_env(
                str(run / "security"),
                auth_secret="test-medulla-secret",
                persist_dir=str(run / "persist"),
            ),
            "MEDULLA_ENCRYPTION_KEY": "test-medulla-encryption-key",
            "MEDULLA_STORAGE_BACKEND": json.dumps(
                {
                    "variant": "object_store",
                    "endpoint": f"{scheme}://db.example/engine",
                    "bucket": "bao:eg/bucket",
                    "root": "bao:eg/root",
                    "durability": "object_store_conditional_put",
                }
            ),
        }
        done = subprocess.run(
            [str(Path(binary).resolve()), "--socket-path", str(run / "engine.sock")],
            env=env,
            capture_output=True,
            text=True,
            timeout=120,
            check=False,
        )
        output = done.stdout + done.stderr
        assert done.returncode != 0, output
        assert "EXTERNAL_PLACEMENT_FORBIDDEN" in output, output
        assert scheme in output.lower(), output
        assert not (run / "engine.sock").exists()
