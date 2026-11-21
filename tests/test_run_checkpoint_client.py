"""Checkpoint facade contract; native isolation needs separate qualification."""

from __future__ import annotations

import asyncio
import hashlib
import time
import uuid
from typing import Any, cast

import pytest
from _client_send_stub import CapturingEngine
from conftest import (
    TEST_AGENT_ID,
    TEST_AUDIENCE,
    TEST_POLICY_VERSION,
    TEST_TENANT,
)
from test_work_item_retry_identity import _submit_request

from medulla import generated as _gen
from medulla.client import EngineResponseError, MedullaClient, WorkItemClient


def client(answer: object) -> tuple[WorkItemClient, CapturingEngine]:
    engine = CapturingEngine(answer)
    return WorkItemClient(cast(MedullaClient, engine)), engine


# spec: MEDULLA-DURABLE-KERNEL-R028.1.3
# spec: MEDULLA-DURABLE-KERNEL-R028.1.3.1
@pytest.mark.no_engine
def test_checkpoint_read_uses_only_native_read_operation() -> None:
    items, engine = client(None)
    assert (
        asyncio.run(
            items.get_checkpoint(
                tenant="t", work_item_id="wi", run_id="run", context_id="context"
            )
        )
        is None
    )
    assert engine.sent == [
        (
            "GetRunCheckpoint",
            {
                "request": {
                    "tenant": "t",
                    "work_item_id": "wi",
                    "run_id": "run",
                    "context_id": "context",
                    "current_claim": None,
                }
            },
        )
    ]


# spec: MEDULLA-DURABLE-KERNEL-R028.1.3
# spec: MEDULLA-DURABLE-KERNEL-R028.1.3.1
@pytest.mark.no_engine
def test_checkpoint_publish_preserves_native_replayed_acknowledgement() -> None:
    receipt: dict[str, Any] = {
        "work_item_id": "wi",
        "run_id": "run",
        "context_id": "context",
        "revision": 1,
        "payload_digest": "a" * 64,
        "changed_work_item_ids": ["wi"],
        "replayed": True,
    }
    items, engine = client(receipt)
    snapshot = {
        "content": "{}",
        "payload_digest": "a" * 64,
        "short_term": [],
        "long_term": [],
    }
    result = asyncio.run(
        items.publish_checkpoint(
            tenant="t",
            work_item_id="wi",
            run_id="run",
            context_id="context",
            worker_id="worker",
            lease_epoch=2,
            fencing_token=3,
            expected_revision=None,
            snapshot=snapshot,
            idempotency_key="request-key",
        )
    )
    assert result == receipt
    method, params = engine.sent[0]
    assert method == "PublishRunCheckpoint"
    assert isinstance(params, dict)
    assert params["request"]["snapshot"] == snapshot
    assert params["request"]["expected_revision"] is None


@pytest.fixture
def served_client(request):
    """A client of a fresh authenticated server, as the sibling served suites use."""
    from decide import test_run_work_item_served as served

    yield from served.authenticated_plan_session(request.getfixturevalue("tmp_path"))


def _admit_run(client, stem: str, context_id: str) -> dict[str, Any]:
    """Submit a run WorkItem on a fresh conversation context and claim it."""
    now = int(time.time() * 1000)
    run_id = "checkpoint-" + stem
    request = _submit_request()
    request.update(
        run_id=run_id,
        work_item_id=None,
        idempotency_key=run_id,
        kind="agent_turn",
        max_tenant_in_flight=4096,
        conversation={
            "schema_version": "1",
            "context_id": context_id,
            "expected_revision": 0,
            "predecessor_run_id": None,
        },
    )
    request["context"].update(
        request_id=run_id,
        trace_id=run_id,
        subject_id=TEST_AGENT_ID,
        tenant_id=TEST_TENANT,
        agent_id=TEST_AGENT_ID,
        audience=TEST_AUDIENCE,
        policy_version=TEST_POLICY_VERSION,
        graph="__commons__",
        scopes=["*"],
        issued_at_ms=now,
        expires_at_ms=now + 60_000,
    )
    item = client.work_items.submit(request)["work_item_id"]
    worker = "worker-" + stem
    claim = client.work_items.claim(
        {
            "schema_version": "1",
            "tenant_ref": TEST_TENANT,
            "work_item_id": item,
            "queue_ref": None,
            "resource_class": None,
            "fairness_group": None,
            "worker_ref": worker,
            "now_ms": int(time.time() * 1000),
            "lease_ms": 30_000,
            "max_tenant_in_flight": 4096,
        },
        idempotency_key=stem + ":claim",
    )
    assert claim["claimed"] is True
    return {
        "work_item_id": item,
        "run_id": run_id,
        "worker_id": worker,
        "lease_epoch": claim["lease_epoch"],
        "fencing_token": claim["fencing_token"],
    }


def _store_memory_document(client, doc_id: str, content: str) -> dict[str, Any]:
    """Accept revision 1 of one memory document through the served sync method."""
    digest = hashlib.sha256(content.encode()).hexdigest()
    outcome = asyncio.run_coroutine_threadsafe(
        _gen.graph.send_sync_memory_snapshot(
            client._client,
            {
                "request": {
                    "request_id": "sync-" + uuid.uuid4().hex,
                    "doc_id": doc_id,
                    "action": {
                        "action": "submit",
                        "content": content,
                        "content_digest": digest,
                        "expected_revision": None,
                    },
                }
            },
        ),
        client._loop,
    ).result(timeout=30)
    head = _gen.graph.decode_sync_memory_snapshot(outcome)
    assert head.outcome == "accepted"
    return {"document_id": doc_id, "revision": head.head.revision, "digest": digest}


# spec: MEDULLA-DURABLE-KERNEL-R028.1.3.2
def test_memory_refs_round_trip_against_a_served_engine(served_client) -> None:
    client = served_client
    stem = uuid.uuid4().hex
    context_id = "context-" + stem
    run = _admit_run(client, stem, context_id)
    short = _store_memory_document(client, "notes/short-" + stem, "short term note")
    long = _store_memory_document(client, "notes/long-" + stem, "long term note")
    content = '{"phase":"step_inflight","next_step":0,"messages":[]}'
    snapshot = {
        "content": content,
        "payload_digest": hashlib.sha256(content.encode()).hexdigest(),
        "short_term": [short],
        "long_term": [long],
    }
    claim_fields = {
        "tenant": TEST_TENANT,
        "work_item_id": run["work_item_id"],
        "run_id": run["run_id"],
        "context_id": context_id,
    }
    receipt = client.work_items.publish_checkpoint(
        **claim_fields,
        worker_id=run["worker_id"],
        lease_epoch=run["lease_epoch"],
        fencing_token=run["fencing_token"],
        expected_revision=None,
        snapshot=snapshot,
        idempotency_key="publish-" + stem,
    )
    assert receipt["replayed"] is False
    read = client.work_items.get_checkpoint(
        **claim_fields,
        current_claim={
            "worker_id": run["worker_id"],
            "lease_epoch": run["lease_epoch"],
            "fencing_token": run["fencing_token"],
        },
    )
    assert read is not None
    assert read["short_term"] == [short]
    assert read["long_term"] == [long]
    assert read["payload_digest"] == snapshot["payload_digest"]

    absent = {**short, "document_id": "notes/absent-" + stem}
    with pytest.raises(EngineResponseError, match="CHECKPOINT_MEMORY_REF_MISSING"):
        client.work_items.publish_checkpoint(
            **claim_fields,
            worker_id=run["worker_id"],
            lease_epoch=run["lease_epoch"],
            fencing_token=run["fencing_token"],
            expected_revision=receipt["revision"],
            snapshot={**snapshot, "short_term": [absent]},
            idempotency_key="absent-" + stem,
        )
