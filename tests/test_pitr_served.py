"""Point-in-time restore through the served engine (MEDULLA-ENGINE-PARITY-R016.2).

A served engine archives its committed batch records into an object-store archive
(`MEDULLA_ARCHIVE_BACKEND`, MEDULLA-ENGINE-PARITY-R016.1). The test writes nodes,
takes a backup bundle, writes more nodes before a chosen time and more after it,
waits for an archive pass, stops the engine, and restores the bundle with
`medulla-server restore --to-time`. A started engine on the restored directory
serves the nodes up to the target and none after it. A target past the end of the
archive refuses with `RECOVERY_TARGET_UNAVAILABLE` and leaves the destination
empty.

The archive endpoint is the loopback object store of
`tests/_loopback_object_store.py`; no store is committed as a fixture.
"""

from __future__ import annotations

import asyncio
import datetime
import json
import os
import subprocess
from pathlib import Path

import msgpack
import pytest
from _loopback_object_store import (
    BUCKET_VARIABLE,
    ENDPOINT_VARIABLE,
    ROOT_VARIABLE,
    LoopbackObjectStore,
)
from conftest import find_server_binary, server_timeout
from test_store_copy_restore import BACKUP_ROOT_ENV, _private_dir
from test_upgrade_store_listing import (
    AGENT,
    SIGNER_KEY,
    _connect,
    _context,
    _Store,
)

from medulla.generated.cluster import send_create_graph
from medulla.generated.graph import send_add_node, send_get_nodes

pytestmark = [
    pytest.mark.no_engine,  # Each case owns its engines on its own stores.
]

GRAPH = "pitr-served"
BUNDLE = "pitr-bundle"
ROOT = "pitr-root"
IN_BUNDLE = ["a-1", "a-2", "a-3"]
BEFORE_TARGET = ["b-1", "b-2"]
AFTER_TARGET = ["c-1", "c-2"]
ARCHIVE_PASS_SECONDS = 10


def _binary() -> str:
    binary = find_server_binary()
    if binary is None:
        pytest.skip("build the full-featured engine binary before this served test")
    return binary


def _archive_env(store: LoopbackObjectStore) -> dict[str, str]:
    """The environment that names the archive: the spec holds variable names only."""

    return {
        ENDPOINT_VARIABLE: store.url,
        BUCKET_VARIABLE: store.bucket,
        ROOT_VARIABLE: ROOT,
        "AWS_ACCESS_KEY_ID": "test",
        "AWS_SECRET_ACCESS_KEY": "test",
        "AWS_REGION": "us-east-1",
        "MEDULLA_ARCHIVE_BACKEND": json.dumps(
            {
                "variant": "object_store",
                "endpoint": ENDPOINT_VARIABLE,
                "bucket": BUCKET_VARIABLE,
                "root": ROOT_VARIABLE,
                "durability": "object_store_conditional_put",
            }
        ),
    }


def _rfc3339(moment: datetime.datetime) -> str:
    return moment.astimezone(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%S.%f")[
        :-3
    ] + "Z"


async def _add_nodes(socket_path: str, nodes: list[str]) -> None:
    client = await _connect(socket_path, _context())
    try:
        for node in nodes:
            await send_add_node(
                client,
                {
                    "node_id": node,
                    "properties_msgpack": msgpack.packb(
                        {"node": node}, use_bin_type=True
                    ),
                },
                GRAPH,
                idempotency_key=f"pitr:add-node:{node}",
            )
    finally:
        await client.close()


async def _read_nodes(socket_path: str) -> set[str]:
    client = await _connect(socket_path, _context())
    try:
        return {node_id for node_id, _ in await send_get_nodes(client, {}, GRAPH)}
    finally:
        await client.close()


class _Journey:
    """One archived history: the bundle, the target time and the archive."""

    def __init__(self, root: Path, binary: str, store: LoopbackObjectStore) -> None:
        self.binary = binary
        self.store = store
        self.backup_root = _private_dir(root / "backups")
        root_dir = root / "source"
        root_dir.mkdir()
        self.source = _Store(root_dir)
        self.target_time = ""
        self.past_target_time = ""
        env = {
            **_archive_env(store),
            BACKUP_ROOT_ENV: str(self.backup_root),
            "MEDULLA_ARCHIVE_INTERVAL_S": str(ARCHIVE_PASS_SECONDS),
        }
        self.source.run(binary, self._write_history, **env)

    async def _write_history(self) -> None:
        socket_path = self.source.socket
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
            await send_create_graph(
                client,
                {"graph_name": GRAPH, "graph_type": "Global"},
                GRAPH,
                idempotency_key="pitr:create-graph",
            )
        finally:
            await client.close()
        await _add_nodes(socket_path, IN_BUNDLE)
        client = await _connect(socket_path, _context())
        try:
            await client.admin.backup(BUNDLE, label="pitr")
        finally:
            await client.close()
        await _add_nodes(socket_path, BEFORE_TARGET)
        await asyncio.sleep(1.2)
        self.target_time = _rfc3339(datetime.datetime.now(datetime.timezone.utc))
        await asyncio.sleep(1.2)
        await _add_nodes(socket_path, AFTER_TARGET)
        # One archive pass runs after the last write and files every record.
        await asyncio.sleep(ARCHIVE_PASS_SECONDS + 3)
        segments = self.store.keys(f"{ROOT}/archive/engine/")
        assert any(key.endswith(".seg") for key in segments), segments

    def restore(self, destination: Path, *target: str) -> subprocess.CompletedProcess:
        bundle = self.backup_root / BUNDLE
        shards = json.loads((bundle / "MANIFEST.json").read_text())["shard_count"]
        destination.mkdir()
        return subprocess.run(
            [
                self.binary,
                "restore",
                "--bundle",
                str(bundle),
                "--persist-dir",
                str(destination),
                "--shards",
                str(shards),
                *target,
            ],
            capture_output=True,
            text=True,
            timeout=server_timeout(),
            env={**os.environ, **_archive_env(self.store)},
        )


@pytest.fixture(scope="module")
def journey(tmp_path_factory) -> _Journey:
    store = LoopbackObjectStore()
    try:
        yield _Journey(tmp_path_factory.mktemp("pitr"), _binary(), store)
    finally:
        store.stop()


@pytest.mark.spec("MEDULLA-ENGINE-PARITY-R016.1", "MEDULLA-ENGINE-PARITY-R016.2")
def test_restore_to_time(journey: _Journey, tmp_path: Path) -> None:
    restored_dir = tmp_path / "restored-data"
    done = journey.restore(restored_dir, "--to-time", journey.target_time)
    assert done.returncode == 0, done.stdout + done.stderr
    assert "restore OK" in done.stdout, done.stdout
    assert "replay OK: segments=" in done.stdout, done.stdout
    records = int(done.stdout.split("replay OK:")[1].split("records=")[1].split()[0])
    assert records >= len(BEFORE_TARGET), done.stdout

    restored_root = tmp_path / "restored"
    restored_root.mkdir()
    restored = _Store(restored_root)
    restored.persist.rmdir()
    restored.persist = restored_dir
    restored.state = journey.source.state

    seen: set[str] = set()

    async def read() -> None:
        seen.update(await _read_nodes(restored.socket))

    restored.run(journey.binary, read)
    assert set(IN_BUNDLE + BEFORE_TARGET) <= seen, seen
    assert not set(AFTER_TARGET) & seen, seen


@pytest.mark.spec("MEDULLA-ENGINE-PARITY-R016.2")
@pytest.mark.parametrize(
    "target",
    [("--to-time", "2099-01-01T00:00:00Z"), ("--to-seq", "1000000000")],
    ids=["time", "sequence"],
)
def test_target_past_archive_refuses(
    journey: _Journey, tmp_path: Path, target: tuple[str, str]
) -> None:
    destination = tmp_path / "refused-data"
    done = journey.restore(destination, *target)
    assert done.returncode == 2, done.stdout + done.stderr
    assert "restore REFUSED" in done.stdout, done.stdout
    assert "recovery_target_unavailable" in done.stdout, done.stdout
    assert list(destination.iterdir()) == [], "a refused restore writes nothing"
