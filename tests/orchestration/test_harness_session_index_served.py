"""MEDULLA-ORCH-VOCAB-R009.101.1 served harness session index.

L4: every call goes through the running engine over the wire client. A committed
``RunTrace`` is indexed through ``index_harness_session_batch``; the engine verifies the
record against the stored trace (revision and digest) and writes the index nodes once.
"""

from __future__ import annotations

import hashlib
import json
import os
from typing import Any

import pytest
from conftest import TEST_TENANT, tenant_request_context

from medulla.client import SyncMedullaClient

_EXCLUDED = {"node_type", "tenant", "run_revision"}


def _canonical(value: Any) -> str:
    if isinstance(value, dict):
        body = ",".join(
            f"{json.dumps(key)}:{_canonical(value[key])}" for key in sorted(value)
        )
        return "{" + body + "}"
    if isinstance(value, list):
        return "[" + ",".join(_canonical(item) for item in value) + "]"
    return json.dumps(value, separators=(",", ":"))


def _trace_digest(properties: dict[str, Any]) -> str:
    """The connector side of ``run_trace_digest`` (session ingest contract)."""
    kept = {k: v for k, v in properties.items() if k not in _EXCLUDED}
    return hashlib.sha256(_canonical(kept).encode()).hexdigest()


def _batch(client: Any, trace_id: str, **overrides: Any) -> dict[str, Any]:
    properties = client.nodes.properties(trace_id)
    batch: dict[str, Any] = {
        "tenant_id": TEST_TENANT,
        "manifest_id": "manifest-1",
        "manifest_revision": 1,
        "manifest_digest": "ab" * 32,
        "declared_total": 2,
        "sync_receipt_id": "receipt-1",
        "sync_receipt_digest": "cd" * 32,
        "source_cursor": "cursor-1",
        "records": [
            {
                "harness_id": "claude-code",
                "adapter_revision": 1,
                "source_session_id": "session-1",
                "run_trace_id": trace_id,
                "run_trace_revision": 1,
                "trace_digest": _trace_digest(properties),
                "source_sequence": 5,
            }
        ],
        "unavailable": [
            {
                "harness_id": "claude-code",
                "source_session_id": "session-2",
                "reason": "transcript unreadable",
            }
        ],
    }
    batch.update(overrides)
    return batch


def _seed_trace(client: Any, trace_id: str) -> None:
    client.nodes.add(
        trace_id, {"node_type": "RunTrace", "status": "completed", "goal": "ship"}
    )
    client.nodes.add(f"{trace_id}:step-1", {"node_type": "RunTraceStep", "index": 0})
    client.nodes.add(f"{trace_id}:step-2", {"node_type": "RunTraceStep", "index": 1})
    client.edges.add(trace_id, f"{trace_id}:step-1", {"relationship": "hasStep"})
    client.edges.add(trace_id, f"{trace_id}:step-2", {"relationship": "hasStep"})


# Verifies MEDULLA-ORCH-VOCAB-R009.101.1: replay is idempotent, changes refuse.
@pytest.mark.spec("MEDULLA-ORCH-VOCAB-R009.101.1")
def test_replay_is_idempotent_and_changed_content_refuses(clean_graph) -> None:
    client = clean_graph
    trace_id = "run:session-index-1"
    _seed_trace(client, trace_id)
    first = client.graph.index_harness_session_batch(_batch(client, trace_id))
    assert (first["indexed"], first["unavailable"], first["unchanged"]) == (1, 1, 0)

    # The identical replay changes nothing and keeps the revision.
    replay = client.graph.index_harness_session_batch(_batch(client, trace_id))
    assert replay["unchanged"] == 2
    assert replay["index_revision"] == first["index_revision"]

    # The same sequence with another trace digest is changed content.
    changed = _batch(client, trace_id)
    changed["records"][0]["trace_digest"] = "00" * 32
    changed["sync_receipt_id"] = "receipt-2"
    with pytest.raises(Exception, match="SESSION_CONTENT_CHANGED"):
        client.graph.index_harness_session_batch(changed)

    # An unknown trace and an unsupported manifest revision are refused by name.
    unknown = _batch(client, trace_id)
    unknown["records"][0]["run_trace_id"] = "run:absent"
    with pytest.raises(Exception, match="SESSION_TRACE_UNKNOWN"):
        client.graph.index_harness_session_batch(unknown)
    with pytest.raises(Exception, match="SESSION_MANIFEST_UNSUPPORTED"):
        client.graph.index_harness_session_batch(
            _batch(client, trace_id, manifest_revision=2)
        )

    # A second tenant sees the same run trace id as absent.
    socket_path = os.environ["GRAPH_SERVICE_SOCKET"]
    other = SyncMedullaClient.connect(
        socket_path=socket_path,
        verified_context=tenant_request_context("tenant-session-other"),
    )
    try:
        foreign = _batch(client, trace_id, tenant_id="tenant-session-other")
        with pytest.raises(Exception, match="SESSION_TRACE_UNKNOWN"):
            other.graph.index_harness_session_batch(foreign)
    finally:
        other.close()


def _search(client: Any, **params: Any) -> dict[str, Any]:
    return client.query.uql("harness_session_search", params)


def _summary(result: dict[str, Any]) -> dict[str, str]:
    row = result["rows"][0]
    assert row["id"] == "summary", result
    return row["provenance"]


def _two_session_batch(client: Any, first: str, second: str, **extra: Any) -> dict:
    batch = _batch(client, first, **extra)
    other = _batch(client, second)["records"][0]
    other["source_session_id"] = "session-b"
    other["source_sequence"] = 3
    batch["records"].append(other)
    return batch


# Verifies MEDULLA-ORCH-VOCAB-R009.101.2: the search covers the full manifest.
@pytest.mark.spec("MEDULLA-ORCH-VOCAB-R009.101.2")
def test_search_covers_manifest_with_unavailable_third_source(clean_graph) -> None:
    client = clean_graph
    first, second = "run:search-1", "run:search-2"
    _seed_trace(client, first)
    _seed_trace(client, second)
    batch = _two_session_batch(client, first, second, declared_total=3)
    client.graph.index_harness_session_batch(batch)

    summary = _summary(_search(client))
    assert [
        summary[name]
        for name in ("declared", "received", "indexed", "unavailable", "pending")
    ] == ["3", "3", "2", "1", "0"]
    entries = [row["provenance"] for row in _search(client)["rows"][1:]]
    assert [entry["source_session_id"] for entry in entries] == [
        "session-1",
        "session-b",
        "session-2",
    ]
    assert entries[2]["status"] == "unavailable"
    assert entries[2]["reason"] == "transcript unreadable"

    # A second batch declaring 4 leaves one declared session pending.
    later = _two_session_batch(
        client,
        first,
        second,
        declared_total=4,
        sync_receipt_id="receipt-2",
        sync_receipt_digest="ee" * 32,
        source_cursor="cursor-2",
    )
    client.graph.index_harness_session_batch(later)
    after = _summary(_search(client))
    assert (after["declared"], after["received"], after["pending"]) == ("4", "3", "1")
    assert after["watermark"] == "cursor-2"
    # Filters narrow the entries; the text matches the session id and the harness.
    narrowed = _search(client, text="SESSION-B")
    assert [row["id"] for row in narrowed["rows"][1:]] == [
        "harnesssession:" + batch["tenant_id"] + ":claude-code:session-b"
    ]


# Verifies MEDULLA-ORCH-VOCAB-R009.101.2: unknown cost is not zero; stale cursors.
@pytest.mark.spec("MEDULLA-ORCH-VOCAB-R009.101.2")
def test_cost_totals_report_unknown_not_zero(clean_graph) -> None:
    client = clean_graph
    first, second = "run:cost-1", "run:cost-2"
    _seed_trace(client, first)
    _seed_trace(client, second)
    client.graph.index_harness_session_batch(_two_session_batch(client, first, second))
    summary = _summary(_search(client))
    assert summary["cost_status"] == "unknown"
    assert summary["cost_reason"] in {"price_missing", "attribution_missing"}
    assert "cost_minor" not in summary

    # A cursor minted before a new batch refuses once the index moved.
    page = _search(client, page_size=1)
    cursor = next(
        warning.removeprefix("next_cursor: ")
        for warning in page["warnings"]
        if warning.startswith("next_cursor: ")
    )
    client.graph.index_harness_session_batch(
        _two_session_batch(
            client,
            first,
            second,
            sync_receipt_id="receipt-9",
            sync_receipt_digest="99" * 32,
        )
    )
    with pytest.raises(Exception, match="SESSION_CURSOR_STALE"):
        _search(client, page_size=1, cursor=cursor)
