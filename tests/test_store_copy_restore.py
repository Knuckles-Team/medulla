"""A copied bundle restores and serves (MEDULLA-DURABLE-KERNEL-R862.3).

The hot-path proof of the recovery rows. A served engine writes two graphs and takes
a backup bundle through the served `Backup` method. The test copies the bundle with
`cp -a`, restores the copy into a new data directory with `medulla-server restore`,
and starts an engine on it: the engine lists both graphs and reads their nodes
(MEDULLA-DURABLE-KERNEL-R862.1). A second case copies the engine's data directory
with `cp -a`, runs `store-upgrade rebind --confirm` on the copy
(MEDULLA-DURABLE-KERNEL-R862.2) and starts an engine on the copy.

A store binds to the device and inode of its files, so every store here is written
in place by this build; nothing is committed as a fixture.
"""

from __future__ import annotations

import json
import os
import subprocess
from pathlib import Path

import msgpack
import pytest
from conftest import find_server_binary, server_timeout
from test_upgrade_store_listing import (
    AGENT,
    SIGNER_KEY,
    _connect,
    _context,
    _Store,
)

from medulla.generated.cluster import (
    decode_list_graphs,
    send_create_graph,
    send_list_graphs,
)
from medulla.generated.graph import send_add_node, send_get_nodes

pytestmark = [
    pytest.mark.no_engine,  # Each case owns its engines on its own stores.
]

GRAPHS = {
    "copy-restore-a": ["a-1", "a-2"],
    "copy-restore-b": ["b-1"],
}
BUNDLE = "copy-restore-bundle"
BACKUP_ROOT_ENV = "MEDULLA_BACKUP_ROOT"


def _binary() -> str:
    binary = find_server_binary()
    if binary is None:
        pytest.skip("build the full-featured engine binary before this served test")
    return binary


def _run(*argv: str) -> subprocess.CompletedProcess:
    completed = subprocess.run(
        list(argv), capture_output=True, text=True, timeout=server_timeout()
    )
    assert completed.returncode == 0, completed.stdout + completed.stderr
    return completed


def _private_dir(path: Path) -> Path:
    path.mkdir()
    path.chmod(0o700)
    return path


async def _write_graphs(socket_path: str) -> None:
    """Bootstrap the System identity, then write both graphs and their nodes."""

    client = await _connect(
        socket_path, _context(roles=[], scopes=["security:bootstrap"])
    )
    try:
        await client.consensus.bootstrap_system_identity(
            agent_id=AGENT, signer_id=AGENT, signer_key=SIGNER_KEY
        )
    finally:
        await client.close()
    client = await _connect(socket_path, _context())
    try:
        for graph, nodes in GRAPHS.items():
            await send_create_graph(
                client,
                {"graph_name": graph, "graph_type": "Global"},
                graph,
                idempotency_key=f"copy-restore:create-graph:{graph}",
            )
            for node in nodes:
                await send_add_node(
                    client,
                    {
                        "node_id": node,
                        "properties_msgpack": msgpack.packb(
                            {"graph": graph, "node": node}, use_bin_type=True
                        ),
                    },
                    graph,
                    idempotency_key=f"copy-restore:add-node:{graph}:{node}",
                )
    finally:
        await client.close()


async def _serves_both_graphs(socket_path: str) -> None:
    """The engine lists both graphs and reads every node written into each."""

    client = await _connect(socket_path, _context())
    try:
        listed = {
            listing.name
            for listing in decode_list_graphs(
                await send_list_graphs(client, {}, "__commons__")
            )
        }
        assert set(GRAPHS) <= listed, listed
        for graph, nodes in GRAPHS.items():
            read = {node_id for node_id, _ in await send_get_nodes(client, {}, graph)}
            assert set(nodes) <= read, (graph, read)
    finally:
        await client.close()


def _written_store(root: Path, binary: str, **extra: str) -> _Store:
    root.mkdir()
    store = _Store(root)
    store.run(binary, lambda: _write_graphs(store.socket), **extra)
    return store


def _store_on(root: Path, persist: Path, state: Path) -> _Store:
    """A `_Store` over an existing data directory and security state."""

    root.mkdir()
    store = _Store(root)
    store.persist.rmdir()
    store.persist = persist
    store.state = state
    return store


@pytest.mark.spec(
    "MEDULLA-DURABLE-KERNEL-R862",
    "MEDULLA-DURABLE-KERNEL-R862.1",
    "MEDULLA-DURABLE-KERNEL-R862.3",
)
def test_copied_bundle_restores_and_serves(tmp_path):
    binary = _binary()
    backup_root = _private_dir(tmp_path / "backups")
    store = _written_store(tmp_path / "source", binary)

    async def back_up() -> None:
        await _serves_both_graphs(store.socket)
        client = await _connect(store.socket, _context())
        try:
            await client.admin.backup(BUNDLE, label="copy-restore")
        finally:
            await client.close()

    store.run(binary, back_up, **{BACKUP_ROOT_ENV: str(backup_root)})
    bundle = backup_root / BUNDLE
    assert (bundle / "MANIFEST.json").is_file()

    copied = tmp_path / "copied-bundle"
    _run("cp", "-a", str(bundle), str(copied))
    shards = json.loads((copied / "MANIFEST.json").read_text())["shard_count"]
    restored_dir = tmp_path / "restored-data"
    restored_dir.mkdir()
    _run(
        binary,
        "restore",
        "--bundle",
        str(copied),
        "--persist-dir",
        str(restored_dir),
        "--shards",
        str(shards),
    )

    restored = _store_on(tmp_path / "restored", restored_dir, store.state)
    restored.run(binary, lambda: _serves_both_graphs(restored.socket))


@pytest.mark.spec("MEDULLA-DURABLE-KERNEL-R862.3")
def test_rebound_copy_serves(tmp_path):
    binary = _binary()
    store = _written_store(tmp_path / "source", binary)

    copy_dir = tmp_path / "copied-data"
    _run("cp", "-a", str(store.persist), str(copy_dir))
    _run(
        binary,
        "store-upgrade",
        "rebind",
        str(copy_dir),
        "--copied-from",
        str(store.persist),
        "--confirm",
    )

    copy = _store_on(tmp_path / "copy", copy_dir, store.state)
    copy.run(binary, lambda: _serves_both_graphs(copy.socket))
    # The source still serves: the rebind rewrote only the copy.
    store.run(binary, lambda: _serves_both_graphs(store.socket))
    assert os.path.isdir(copy_dir)
