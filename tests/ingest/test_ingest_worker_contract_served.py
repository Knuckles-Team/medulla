"""The engine methods the SDK ingest runner relies on: MEDULLA-INGEST-WORKER-R001.14.

The Python client drives the full runner sequence: register a worker, ask for a slot
grant, claim one ingest job under it, ack it after execution. A body killed before
its ack sends nothing, and the job redelivers once its lease lapses.
"""

from __future__ import annotations

import asyncio
import time
import uuid

import pytest

from medulla.client import MedullaClient

from .test_ingest_queue_served import _client, _envelope


async def _enqueue(client: MedullaClient) -> str:
    receipt = await client._send(
        "IngestEnqueue", {"envelope": _envelope(f"job-{uuid.uuid4().hex[:10]}")}
    )
    return str(receipt["task_id"])


async def _claim_until(
    client: MedullaClient, task_id: str, worker: str, grant_id: str, lease_ms: int
):
    deadline = time.monotonic() + 15.0
    while time.monotonic() < deadline:
        claimed = await client.task_queue.claim(
            worker_id=worker, grant_id=grant_id, lease_ms=lease_ms
        )
        if claimed is None:
            await asyncio.sleep(0.05)
        elif claimed.task_id == task_id:
            return claimed
        else:
            await client.task_queue.ack(
                task_id=claimed.task_id, worker_id=worker, outcome="Succeeded"
            )
    raise AssertionError(f"task {task_id} was not delivered")


@pytest.mark.spec("MEDULLA-INGEST-WORKER-R001.14")
@pytest.mark.asyncio
async def test_runner_call_sequence() -> None:
    client = await _client()
    try:
        worker = f"runner-{uuid.uuid4().hex[:8]}"
        record = await client.task_queue.register_worker(
            worker_id=worker,
            host_iri="host:alpha",
            cores=8,
            memory_bytes=8 * 1024**3,
            lanes_supported=["general"],
        )
        assert record.worker_id == worker
        admission = await client.task_queue.admit_worker_task(
            worker_id=worker, requested_max=1
        )
        assert hasattr(admission, "Granted"), admission
        grant = admission.Granted
        assert grant.slots >= 1

        # Claim, execute, then ack: the ack follows execution.
        task_id = await _enqueue(client)
        claimed = await _claim_until(client, task_id, worker, grant.grant_id, 60_000)
        assert claimed.attempt == 1
        acked = await client.task_queue.ack(
            task_id=task_id, worker_id=worker, outcome="Succeeded"
        )
        assert acked.task_id == task_id

        # A body killed before its ack sends no ack: the job redelivers once the lease
        # lapses, as attempt 2, and a late ack of the first lease is refused.
        killed = await _enqueue(client)
        first = await _claim_until(client, killed, worker, grant.grant_id, 300)
        assert first.attempt == 1
        await asyncio.sleep(0.8)
        again = await _claim_until(client, killed, worker, grant.grant_id, 60_000)
        assert again.attempt == 2
        with pytest.raises(Exception, match="TASK_ACK_"):
            await client.task_queue.ack(
                task_id=killed, worker_id=f"{worker}-stale", outcome="Succeeded"
            )
        await client.task_queue.ack(
            task_id=killed, worker_id=worker, outcome="Succeeded"
        )
    finally:
        await client.close()
