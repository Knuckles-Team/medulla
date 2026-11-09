"""A store written by the last production release opens on this build (R070.4).

Spec row: MEDULLA-DURABLE-KERNEL-R070.4. The release pinned in
`tests/fixtures/production_release.json` writes a store with its own engine and
client. That store holds tenant graphs on every graph shard, committed
transactions with signed authority records, and a blob. This build then opens
the same data directory with no offline step. Automatic migration must upgrade
every store, and every node, edge, graph and blob must read back unchanged. A
new write must commit on every graph. A second start must find nothing to
migrate and still serve the same data.

`scripts/fetch_production_engine.py <dir>` prepares the production release:
`<dir>/server` is its engine and `<dir>/venv/bin/python` holds its client. Point
`MEDULLA_PRODUCTION_SERVER_BINARY` and `MEDULLA_PRODUCTION_PYTHON` at them. With
`MEDULLA_REQUIRE_PRODUCTION_UPGRADE=1`, as the CI gate sets it, a missing input
fails the test instead of skipping it.

A store binds to the device and inode of its files, so the production engine
writes the store in place; a copied store is refused (MEDULLA-DURABLE-KERNEL-R862).
The migration stages its scratch copy inside the data directory, which must not
be under `/tmp`. Run this test with `--basetemp` outside it.
"""

from __future__ import annotations

import asyncio
import json
import os
import re
import subprocess
import sys
from pathlib import Path
from typing import Any, NoReturn

import pytest
from conftest import (
    find_server_binary,
    server_timeout,
    spawn_server,
    stop_server,
    wait_for_server,
)

from medulla.client import MedullaClient

pytestmark = [
    pytest.mark.no_engine,  # The case owns its engines on its own store.
    pytest.mark.spec("MEDULLA-DURABLE-KERNEL-R070.4"),
]

HERE = Path(__file__).resolve().parent
PIN = HERE / "fixtures" / "production_release.json"
WRITER = HERE / "_production_store_writer.py"
SERVER_ENV = "MEDULLA_PRODUCTION_SERVER_BINARY"
PYTHON_ENV = "MEDULLA_PRODUCTION_PYTHON"
REQUIRE_ENV = "MEDULLA_REQUIRE_PRODUCTION_UPGRADE"
BLOB = b"medulla upgrade proof blob\n" * 64  # The writer stores the same bytes.
ANSI = re.compile(r"\x1b\[[0-9;]*m")

AGENT = "service:test-suite"
SETTINGS: dict[str, Any] = {
    "agent": AGENT,
    "signer_key": "test-operation-signer-key",  # sanitizer:ignore
    "auth_secret": "test-production-upgrade-secret",  # sanitizer:ignore
    "audience": "medulla-test",
    "tenant": "tenant:test",
    "policy_version": "policy:test",
    "encryption_key": "test-production-upgrade-key",  # sanitizer:ignore
    "context": {
        "principal": AGENT,
        "tenant": "tenant:test",
        "audience": "medulla-test",
        "agent_id": AGENT,
        "roles": ["test"],
        "scopes": ["*"],
        "policy_version": "policy:test",
        "delegation": [],
    },
}


def _missing(reason: str) -> NoReturn:
    if os.environ.get(REQUIRE_ENV, "").strip() == "1":
        raise pytest.fail.Exception(f"{REQUIRE_ENV}=1 but {reason}")
    raise pytest.skip.Exception(reason)


def _input(name: str) -> str:
    value = os.environ.get(name, "").strip()
    if not value or not os.path.isfile(value):
        _missing(f"{name} does not name a file; run scripts/fetch_production_engine.py")
    return value


def _env(root: Path) -> dict[str, str]:
    inherited = {
        key: value
        for key, value in os.environ.items()
        if not key.startswith(("GRAPH_SERVICE_", "MEDULLA_", "EPISTEMIC_GRAPH_"))
    }
    signers = {
        AGENT: {
            "key": SETTINGS["signer_key"],
            "allowed_roles": ["test"],
            "may_grant_system": True,
        }
    }
    return {
        **inherited,
        "GRAPH_SERVICE_AUTH_SECRET": SETTINGS["auth_secret"],
        "MEDULLA_REQUIRE_OIDC": "false",
        "MEDULLA_AUDIENCE": SETTINGS["audience"],
        "MEDULLA_TENANT": SETTINGS["tenant"],
        "MEDULLA_POLICY_VERSION": SETTINGS["policy_version"],
        "MEDULLA_SECURITY_STATE_DIR": str(root / "security"),
        "MEDULLA_ENCRYPTION_KEY": SETTINGS["encryption_key"],
        "MEDULLA_SIGNER_KEYS_JSON": json.dumps(signers),
    }


def _write_production_store(root: Path) -> dict:
    """Run the production engine and client; return the writer's manifest."""

    settings = root / "settings.json"
    settings.write_text(json.dumps(SETTINGS), encoding="utf-8")
    written = subprocess.run(
        [
            _input(PYTHON_ENV),
            "-I",
            str(WRITER),
            str(PIN),
            _input(SERVER_ENV),
            str(root),
            str(settings),
        ],
        capture_output=True,
        text=True,
        timeout=server_timeout() + 600,
    )
    assert written.returncode == 0, written.stdout + written.stderr
    return json.loads(written.stdout.strip().splitlines()[-1])


async def _connect(socket: str, graph: str = "__commons__") -> MedullaClient:
    return await MedullaClient.connect(
        socket_path=socket,
        auth_secret=SETTINGS["auth_secret"],
        verified_context=SETTINGS["context"],
        graph_name=graph,
    )


async def _graph_intact(socket: str, graph: str, written: dict) -> None:
    client = await _connect(socket, graph)
    try:
        for node, properties in written["nodes"].items():
            assert await client.nodes.has(node), f"{graph}: node {node} is missing"
            stored = await client.nodes.properties(node)
            assert stored is not None
            assert {key: stored.get(key) for key in properties} == properties, (
                graph,
                node,
            )
        for source, target in written["edges"]:
            assert await client.edges.has(source, target), (
                f"{graph}: edge {source}->{target}"
            )
    finally:
        await client.close()


async def _intact(socket: str, manifest: dict, extra_nodes: bool) -> None:
    client = await _connect(socket)
    try:
        listed = {
            entry["name"] if isinstance(entry, dict) else str(entry)
            for entry in await client.tenants.list()
        }
        missing = set(manifest["graphs"]) - listed
        assert not missing, f"graphs missing after the upgrade: {sorted(missing)}"
        assert await client.blob.fetch(manifest["blob"]) == BLOB
    finally:
        await client.close()
    for graph, written in manifest["graphs"].items():
        if extra_nodes:
            written = {
                **written,
                "nodes": {
                    **written["nodes"],
                    f"{graph}-after": {"kind": "AfterUpgrade"},
                },
            }
        await _graph_intact(socket, graph, written)


async def _write_after_upgrade(socket: str, manifest: dict) -> None:
    """Every graph accepts a new write; it carries the current authority protocol."""

    client = await _connect(socket)
    try:
        for graph in manifest["graphs"]:
            txn = await client.txn.begin(graph=graph)
            await client.txn.add_node(
                txn, f"{graph}-after", {"kind": "AfterUpgrade"}, graph=graph
            )
            assert await client.txn.commit(
                txn, idempotency_key=f"{graph}-after-upgrade"
            )
    finally:
        await client.close()


def _serve(binary: str, root: Path, check, log_name: str) -> str:
    socket = str(root / "candidate.sock")
    if os.path.exists(socket):
        os.remove(socket)
    log = root / log_name
    process = spawn_server(
        [binary, "--socket-path", socket, "--persist-dir", str(root / "persist")],
        env=_env(root),
        log_path=log,
    )
    try:
        wait_for_server(process, socket, name="candidate engine", log_path=log)
        asyncio.run(check(socket))
    finally:
        stop_server(process, name="candidate engine", log_path=log)
    return ANSI.sub("", log.read_text(errors="replace"))


def test_a_production_store_migrates_and_serves_with_data_intact(tmp_path):
    binary = find_server_binary()
    if binary is None:
        _missing("build the full-featured engine binary before this served test")
    if str(tmp_path).startswith("/tmp/"):
        _missing("the migration refuses a store under /tmp; pass --basetemp outside it")
    root = tmp_path / "store"
    root.mkdir()
    manifest = _write_production_store(root)
    assert manifest["version"] == json.loads(PIN.read_text(encoding="utf-8"))["version"]

    async def first_open(socket: str) -> None:
        await _intact(socket, manifest, extra_nodes=False)
        await _write_after_upgrade(socket, manifest)
        await _intact(socket, manifest, extra_nodes=True)

    log = _serve(binary, root, first_open, "candidate-first.log")
    assert "STORE_AUTO_MIGRATE_FAILED" not in log, log[-4000:]

    async def second_open(socket: str) -> None:
        await _intact(socket, manifest, extra_nodes=True)

    log = _serve(binary, root, second_open, "candidate-second.log")
    assert "store migration: upgrading store" not in log, log[-4000:]


if __name__ == "__main__":
    sys.exit(pytest.main([__file__, "-q"]))
