"""Served proof for MEDULLA-DURABLE-KERNEL-R700.3: the retention cleanup method.

One real native engine holds aged documents, one of them under a legal hold.
The served `RetentionCleanupRun` is called with `dry_run` true and then false.
The sweep re-enters this same method (`retention_cleanup::run_tick`), so the
report the method answers is the report the sweep acts on: the dry run changes
nothing, the real run answers the identical report and removes exactly what the
dry run named, and the held document survives.
"""

import json

import msgpack
import pytest

pytestmark = [
    pytest.mark.no_engine,  # Each case owns its real isolated process.
    pytest.mark.spec("MEDULLA-DURABLE-KERNEL-R700.3"),
]

GRAPH = "__commons__"
POLICY = {"age_ms": 1_000, "batch": 100, "holds": ["live_reference", "legal_hold"]}


def _value(result):
    """A served result is a value, or the MessagePack of one."""
    if isinstance(result, (bytes, bytearray)):
        return msgpack.unpackb(result, raw=False)
    return result


def _add_document(rpc, ok, sock, node_id, *, legal_hold=False):
    row = {"type": "Document", "updated_at": 1}
    if legal_hold:
        row["metadata"] = {"legal_hold": True}
    ok(
        rpc(
            sock,
            "AddNode",
            {"node_id": node_id, "properties_msgpack": msgpack.packb(row)},
            key=f"add-{node_id}",
        )
    )


def _run(rpc, ok, sock, key, *, dry_run, cursor=None):
    return _value(
        ok(
            rpc(
                sock,
                "RetentionCleanupRun",
                {"request": {"graph": GRAPH, "dry_run": dry_run, "cursor": cursor}},
                key=key,
            )
        )
    )


def test_run_method_matches_sweep(tmp_path):
    """Verifies MEDULLA-DURABLE-KERNEL-R700.3.

    The served method's dry run and real run answer one report, and the real run
    removes the aged documents but not the one under a legal hold.
    """
    from test_native_no_auth_served import ok, rpc, server

    env = {"MEDULLA_RETENTION_CLEANUP": json.dumps(POLICY)}
    with server(tmp_path, extra_env=env) as sock:
        _add_document(rpc, ok, sock, "doc-1")
        _add_document(rpc, ok, sock, "doc-2")
        _add_document(rpc, ok, sock, "doc-3", legal_hold=True)

        dry = _run(rpc, ok, sock, "retention-dry", dry_run=True)
        assert dry["removed"] == 2
        assert dry["held_legal_hold"] == 1
        for node_id in ("doc-1", "doc-2", "doc-3"):
            assert ok(rpc(sock, "HasNode", {"node_id": node_id})) is True

        real = _run(rpc, ok, sock, "retention-real", dry_run=False)
        assert real == dry
        assert ok(rpc(sock, "HasNode", {"node_id": "doc-1"})) is False
        assert ok(rpc(sock, "HasNode", {"node_id": "doc-2"})) is False
        assert ok(rpc(sock, "HasNode", {"node_id": "doc-3"})) is True

        # A second step starts from the graph's start again: only the held
        # document is left, and it is kept.
        again = _run(rpc, ok, sock, "retention-again", dry_run=False)
        assert again["removed"] == 0
        assert again["held_legal_hold"] == 1
        assert ok(rpc(sock, "HasNode", {"node_id": "doc-3"})) is True
