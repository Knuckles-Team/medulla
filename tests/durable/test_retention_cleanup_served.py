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


def _state(rpc, ok, sock, node_ids):
    """Whether each node is present, as a dict."""
    return {n: ok(rpc(sock, "HasNode", {"node_id": n})) for n in node_ids}


def _refused(rpc, sock, key, **kwargs):
    response = rpc(
        sock,
        "RetentionCleanupRun",
        {"request": {"graph": kwargs.get("graph", GRAPH), "dry_run": False}},
        key=key,
    )
    assert response.get("error"), response
    return str(response["error"])


@pytest.mark.spec("MEDULLA-DURABLE-KERNEL-R700.4")
@pytest.mark.parametrize(
    ("policy", "code"),
    [
        (
            {"age_ms": 1_000, "batch": 100, "holds": []},
            "RETENTION_HOLD_CHECK_REQUIRED",
        ),
        (
            {"age_ms": 0, "batch": 100, "holds": ["legal_hold"]},
            "RETENTION_POLICY_INVALID",
        ),
        (
            {"age_ms": 1_000, "batch": 0, "holds": ["legal_hold"]},
            "RETENTION_POLICY_INVALID",
        ),
        (
            {"age_ms": 1_000, "batch": 10_001, "holds": ["legal_hold"]},
            "RETENTION_POLICY_INVALID",
        ),
    ],
)
def test_refusals_leave_state_unchanged(tmp_path, policy, code):
    """Verifies MEDULLA-DURABLE-KERNEL-R700.4.

    A policy with no hold check, zero age or an out-of-range batch refuses with its
    code and leaves the graph unchanged; so does a step naming another graph.
    """
    from test_native_no_auth_served import ok, rpc, server

    env = {"MEDULLA_RETENTION_CLEANUP": json.dumps(policy)}
    nodes = ("doc-1", "doc-2", "doc-3")
    with server(tmp_path, extra_env=env) as sock:
        _add_document(rpc, ok, sock, "doc-1")
        _add_document(rpc, ok, sock, "doc-2")
        _add_document(rpc, ok, sock, "doc-3", legal_hold=True)
        before = _state(rpc, ok, sock, nodes)
        assert all(before.values())

        refusal = _refused(rpc, sock, "retention-invalid-policy")
        assert code in refusal, refusal
        assert _state(rpc, ok, sock, nodes) == before


@pytest.mark.spec("MEDULLA-DURABLE-KERNEL-R700.4")
def test_cross_graph_step_refuses_and_changes_nothing(tmp_path):
    """Verifies MEDULLA-DURABLE-KERNEL-R700.4: a step never runs over another graph."""
    from test_native_no_auth_served import ok, rpc, server

    env = {"MEDULLA_RETENTION_CLEANUP": json.dumps(POLICY)}
    nodes = ("doc-1", "doc-2", "doc-3")
    with server(tmp_path, extra_env=env) as sock:
        _add_document(rpc, ok, sock, "doc-1")
        _add_document(rpc, ok, sock, "doc-2")
        _add_document(rpc, ok, sock, "doc-3", legal_hold=True)
        refusal = _refused(rpc, sock, "retention-foreign", graph="__other_tenant__")
        assert "INVALID_ARGUMENT" in refusal, refusal
        assert all(_state(rpc, ok, sock, nodes).values())


def _records(rpc, ok, sock):
    rows = _value(
        ok(
            rpc(
                sock,
                "GetNodesByLabel",
                {"label": "RetentionCleanupRecord", "limit": 1_000},
            )
        )
    )
    records = []
    for _node_id, properties in rows:
        properties = _value(properties)
        records.append(properties)
    return records


@pytest.mark.spec("MEDULLA-DURABLE-KERNEL-R700.5")
def test_record_reproduces_report(tmp_path):
    """Verifies MEDULLA-DURABLE-KERNEL-R700.5.

    A dry run writes no record. A real step writes one `RetentionCleanupRecord` in
    its own batch, and that record reproduces the report the step returned.
    """
    from test_native_no_auth_served import ok, rpc, server

    env = {"MEDULLA_RETENTION_CLEANUP": json.dumps(POLICY)}
    with server(tmp_path, extra_env=env) as sock:
        _add_document(rpc, ok, sock, "doc-1")
        _add_document(rpc, ok, sock, "doc-2")
        _add_document(rpc, ok, sock, "doc-3", legal_hold=True)

        dry = _run(rpc, ok, sock, "record-dry", dry_run=True)
        assert _records(rpc, ok, sock) == []

        real = _run(rpc, ok, sock, "record-real", dry_run=False)
        assert real == dry
        records = _records(rpc, ok, sock)
        assert len(records) == 1
        record = records[0]
        assert record["graph"] == GRAPH
        assert record["scanned"] == real["scanned"]
        assert record["removed"] == real["removed"]
        assert record["held_live_reference"] == real["held_live_reference"]
        assert record["held_legal_hold"] == real["held_legal_hold"]
        assert record["cursor_start"] is None
        assert record["cursor_end"] == real["next_cursor"]
        assert len(record["policy_digest"]) == 64
        assert all(c in "0123456789abcdef" for c in record["policy_digest"])


def _add_row(rpc, ok, sock, node_id, row):
    ok(
        rpc(
            sock,
            "AddNode",
            {"node_id": node_id, "properties_msgpack": msgpack.packb(row)},
            key=f"add-{node_id}",
        )
    )


@pytest.mark.spec("MEDULLA-DURABLE-KERNEL-R700.6")
def test_held_and_referenced_nodes_survive(tmp_path):
    """Verifies MEDULLA-DURABLE-KERNEL-R700.6.

    Aged documents and an aged orphan are removed; a referenced document and a document
    under a legal hold survive, with the node that references the first.
    """
    from test_native_no_auth_served import ok, rpc, server

    env = {"MEDULLA_RETENTION_CLEANUP": json.dumps(POLICY)}
    with server(tmp_path, extra_env=env) as sock:
        for node_id in ("doc-aged-1", "doc-aged-2", "doc-referenced"):
            _add_row(rpc, ok, sock, node_id, {"type": "Document", "updated_at": 1})
        _add_row(
            rpc,
            ok,
            sock,
            "doc-held",
            {"type": "Document", "updated_at": 1, "metadata": {"legal_hold": True}},
        )
        _add_row(rpc, ok, sock, "orphan-aged", {"type": "Thing", "created_at": 1})
        _add_row(rpc, ok, sock, "referrer", {"type": "Thing"})
        ok(
            rpc(
                sock,
                "AddEdge",
                {
                    "source_id": "referrer",
                    "target_id": "doc-referenced",
                    "properties_msgpack": msgpack.packb({"type": "cites"}),
                },
                key="edge-referrer",
            )
        )

        removed = 0
        cursor = None
        for step in range(5):
            report = _run(rpc, ok, sock, f"steps-{step}", dry_run=False, cursor=cursor)
            removed += report["removed"]
            cursor = report["next_cursor"]
            if report["scanned"] == 0 or cursor is None:
                break
        assert removed == 3

        gone = ("doc-aged-1", "doc-aged-2", "orphan-aged")
        kept = ("doc-referenced", "doc-held", "referrer")
        assert not any(_state(rpc, ok, sock, gone).values())
        assert all(_state(rpc, ok, sock, kept).values())
