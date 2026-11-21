"""Served ingest enqueue: MEDULLA-INGEST-WORKER-R001.4 and R001.4.1.

`IngestEnqueue` maps one ingest job onto the reserved served task queue with a
deterministic task id. A duplicate returns the first receipt and writes nothing, a
forged tenant or a credential-bearing target is refused with no task written, and the
task a worker claims carries the receipt's task id.
"""

from __future__ import annotations

import asyncio
import os
import time
import uuid
from typing import Any

import pytest
from conftest import TEST_TENANT, request_context

from medulla.client import MedullaClient

pytestmark = [
    pytest.mark.spec("MEDULLA-INGEST-WORKER-R001.4"),
    pytest.mark.spec("MEDULLA-INGEST-WORKER-R001.4.1"),
]


def _envelope(job_id: str, tenant: str = TEST_TENANT, target: str = "doc://a") -> dict[str, Any]:
    return {
        "job_id": job_id,
        "tenant": tenant,
        "corpus_key": "corpus-a",
        "kind": "document",
        "target": target,
        "attempt": 1,
    }


async def _client() -> MedullaClient:
    return await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
    )


async def _claim(client: MedullaClient, worker: str) -> dict[str, Any] | None:
    return await client._send(
        "TaskQueueClaim",
        {"worker_id": worker, "grant_id": "grant-1", "lease_ms": 60_000},
    )


# spec: MEDULLA-INGEST-WORKER-R001.4.1
@pytest.mark.asyncio
async def test_duplicate_enqueue_is_one_task() -> None:
    client = await _client()
    try:
        job_id = f"job-{uuid.uuid4().hex[:10]}"
        first = await client._send("IngestEnqueue", {"envelope": _envelope(job_id)})
        assert first["deduplicated"] is False
        assert len(first["task_id"]) == 64
        assert first["queue"]

        again = await client._send("IngestEnqueue", {"envelope": _envelope(job_id)})
        assert again["deduplicated"] is True
        assert again["task_id"] == first["task_id"]
        assert again["queue"] == first["queue"]
        assert again["partition"] == first["partition"]

        # Exactly one claimable task carries the receipt's id (the queue is shared with
        # other tests of the tenant, so drain it and count the matches).
        claimed_ids: list[str] = []
        while (claimed := await _claim(client, "ingest-worker-1")) is not None:
            claimed_ids.append(claimed["task_id"])
        assert claimed_ids.count(first["task_id"]) == 1
    finally:
        await client.close()


async def _claim_task(client: MedullaClient, task_id: str, worker: str) -> dict[str, Any]:
    """Claim until `task_id` arrives, acking every other task of the shared queue."""
    deadline = time.monotonic() + 10.0
    while time.monotonic() < deadline:
        claimed = await _claim(client, worker)
        if claimed is None:
            await asyncio.sleep(0.05)
        elif claimed["task_id"] == task_id:
            return claimed
        else:
            await client._send(
                "TaskQueueAck",
                {
                    "task_id": claimed["task_id"],
                    "worker_id": worker,
                    "outcome": "Succeeded",
                    "detail": "",
                },
            )
    raise AssertionError(f"task {task_id} was not delivered")


# spec: MEDULLA-INGEST-WORKER-R001.9
@pytest.mark.spec("MEDULLA-INGEST-WORKER-R001.9")
@pytest.mark.asyncio
async def test_failed_job_retries_then_dead_letters() -> None:
    client = await _client()
    try:
        current = await client._send("GetSchedulerConfig", {})
        fast = {
            **current,
            "ingest_retry": {
                "max_attempts": 5,
                "base_delay_ms": 20,
                "max_delay_ms": 200,
                "jitter_milli": 200,
            },
        }
        await client._send("SetSchedulerConfig", {"config": fast})
        try:
            receipt = await client._send(
                "IngestEnqueue", {"envelope": _envelope(f"job-{uuid.uuid4().hex[:10]}")}
            )
            task_id = receipt["task_id"]
            for attempt in range(1, 6):
                claimed = await _claim_task(client, task_id, "ingest-worker-1")
                assert claimed["attempt"] == attempt
                acked = await client._send(
                    "TaskQueueAck",
                    {
                        "task_id": task_id,
                        "worker_id": "ingest-worker-1",
                        "outcome": "Failed",
                        "detail": "parse failed",
                    },
                )
                assert acked["task_id"] == task_id
            # Five deliveries exhausted the policy: the task is failed for good with the
            # dead-letter reason, and no sixth delivery follows.
            record = await client.jobs.status(task_id)
            assert record["state"]["Failed"]["reason"] == "INGEST_RETRIES_EXHAUSTED"
            await asyncio.sleep(0.5)
            while (extra := await _claim(client, "ingest-worker-1")) is not None:
                assert extra["task_id"] != task_id
        finally:
            await client._send("SetSchedulerConfig", {"config": current})
    finally:
        await client.close()


# spec: MEDULLA-INGEST-WORKER-R001.4
@pytest.mark.asyncio
async def test_forged_tenant_and_credential_target_are_refused() -> None:
    client = await _client()
    try:
        with pytest.raises(Exception, match="INGEST_TENANT_MISMATCH"):
            await client._send(
                "IngestEnqueue",
                {"envelope": _envelope(f"job-{uuid.uuid4().hex[:10]}", tenant="other-tenant")},
            )
        with pytest.raises(Exception, match="INGEST_JOB_INVALID"):
            await client._send(
                "IngestEnqueue",
                {
                    "envelope": _envelope(
                        f"job-{uuid.uuid4().hex[:10]}", target="https://user:secret@host/x"
                    )
                },
            )
    finally:
        await client.close()
