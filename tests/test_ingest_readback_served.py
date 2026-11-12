"""Served `VerifyIngestReadback`: independent readback of an ingest write.

MEDULLA-REPO-INGEST-R002.100 over a dedicated engine where row-level security
is enforced, so a caller can be denied a row the writer (a System identity)
wrote. The reader is a registered non-System principal.
"""

from __future__ import annotations

import hashlib
import json
import os
import struct

import pytest
from conftest import (
    TEST_AGENT_ID,
    TEST_SIGNER_KEY,
    bootstrap_context,
    find_server_binary,
    request_context,
    spawn_server,
    stop_server,
    strict_server_env,
    wait_for_server,
)

from medulla.client import EngineResponseError, SyncMedullaClient

RUST_DIR = os.path.abspath(os.path.join(os.path.dirname(__file__), ".."))
SERVER_BIN = find_server_binary() or os.path.join(
    RUST_DIR, "target", "debug", "medulla-server"
)
SECRET = "test-readback-secret"  # sanitizer:ignore
READER = "service:worker-2"
GRAPH = "__commons__"


def _framed(domain: bytes, fields: list[bytes]) -> str:
    """Mirror of `Digest256::framed` (medulla-types contract/crypto.rs)."""
    hasher = hashlib.sha256()
    hasher.update(b"eg/framed-sha256/v1\0")
    hasher.update(struct.pack(">I", len(domain)) + domain)
    hasher.update(struct.pack(">I", len(fields)))
    for field in fields:
        hasher.update(struct.pack(">Q", len(field)) + field)
    return hasher.hexdigest()


def _content_digest(properties: dict) -> str:
    compact = json.dumps(
        properties, sort_keys=True, separators=(",", ":"), ensure_ascii=False
    )
    return _framed(b"eg/ingest-readback/node/v1", [compact.encode()])


@pytest.fixture(scope="module")
def readback_server(tmp_path_factory):
    runtime = tmp_path_factory.mktemp("readback")
    sock = str(runtime / "readback.sock")
    persist_dir = str(runtime / "persist")
    os.makedirs(persist_dir, exist_ok=True)
    log_path = runtime / "server.log"
    proc = spawn_server(
        [SERVER_BIN, "--socket-path", sock],
        env={
            **os.environ,
            **strict_server_env(
                str(runtime / "security"), auth_secret=SECRET, persist_dir=persist_dir
            ),
        },
        log_path=log_path,
    )
    wait_for_server(proc, sock, name="readback server", log_path=log_path)
    bootstrap = SyncMedullaClient.connect(
        socket_path=sock, auth_secret=SECRET, verified_context=bootstrap_context()
    )
    try:
        bootstrap.consensus.bootstrap_system_identity(
            agent_id=TEST_AGENT_ID,
            signer_id=TEST_AGENT_ID,
            signer_key=TEST_SIGNER_KEY,
        )
    finally:
        bootstrap.close()
    writer = _client(sock, TEST_AGENT_ID)
    try:
        writer.rbac.add_role("commons-access")
        writer.rbac.add_grant("commons-access", {"Graph": GRAPH}, "Read")
        writer.rbac.add_grant("commons-access", {"Graph": GRAPH}, "Write")
        writer.consensus.register_identity(
            READER,
            "Agent",
            ["team:test"],
            ["commons-access"],
            signer_id=TEST_AGENT_ID,
            signer_key=TEST_SIGNER_KEY,
        )
    finally:
        writer.close()
    yield sock
    stop_server(proc, name="readback server", log_path=log_path)


def _client(sock: str, agent_id: str) -> SyncMedullaClient:
    return SyncMedullaClient.connect(
        socket_path=sock,
        auth_secret=SECRET,
        graph_name=GRAPH,
        verified_context=request_context(agent_id=agent_id),
    )


def _expectation(reader: SyncMedullaClient, node_id: str) -> dict:
    properties = reader.nodes.properties(node_id)
    return {
        "id": node_id,
        "content_digest": _content_digest(properties),
        "revision": int(properties.get("revision", 0)),
    }


def _verify(reader: SyncMedullaClient, results: list[dict], revision: int = 0) -> dict:
    request = {
        "write_receipt_digest": _framed(b"test/write-receipt", [b"receipt-1"]),
        "expected_graph_revision": revision,
        "results": results,
    }
    return reader._send("VerifyIngestReadback", {"request": request})


@pytest.mark.spec("MEDULLA-REPO-INGEST-R002.100")
def test_matched_readback_and_each_refusal(readback_server):
    writer = _client(readback_server, TEST_AGENT_ID)
    reader = _client(readback_server, READER)
    try:
        writer.nodes.add(
            "rb:a", {"type": "Fact", "_visibility": "public", "revision": 2}
        )
        writer.nodes.add("rb:b", {"type": "Fact", "_visibility": "public"})
        writer.nodes.add(
            "rb:hidden",
            {"type": "Fact", "_owner": "service:worker-1", "_visibility": "private"},
        )
        good = [_expectation(reader, "rb:a"), _expectation(reader, "rb:b")]
        before = reader.nodes.count()

        receipt = _verify(reader, good)
        assert receipt["verdict"] == "matched"
        assert receipt["principal"] == READER
        assert sorted(receipt["matched_ids"]) == ["rb:a", "rb:b"]
        assert receipt["mismatched_ids"] == []
        assert reader.nodes.count() == before + 1
        after_match = reader.nodes.count()

        with pytest.raises(EngineResponseError, match="READBACK_STALE"):
            _verify(reader, good, revision=2**62)
        with pytest.raises(EngineResponseError, match="READBACK_DENIED"):
            _verify(reader, [good[0], {**good[0], "id": "rb:hidden"}])
        with pytest.raises(EngineResponseError, match="READBACK_PARTIAL"):
            _verify(reader, [good[0], {**good[0], "id": "rb:absent"}])
        with pytest.raises(EngineResponseError, match="READBACK_MISMATCH"):
            _verify(reader, [{**good[1], "content_digest": "0" * 64}])
        with pytest.raises(EngineResponseError, match="READBACK_MISMATCH"):
            _verify(reader, [{**good[0], "revision": 9}])
        assert reader.nodes.count() == after_match
    finally:
        writer.close()
        reader.close()
