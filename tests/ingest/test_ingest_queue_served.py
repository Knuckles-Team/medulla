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


def _envelope(
    job_id: str,
    tenant: str = TEST_TENANT,
    target: str = "doc://a",
    kind: str = "document",
) -> dict[str, Any]:
    return {
        "job_id": job_id,
        "tenant": tenant,
        "corpus_key": "corpus-a",
        "kind": kind,
        "target": target,
        "attempt": 1,
    }


async def _client() -> MedullaClient:
    return await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
    )


_REGISTERED: set[str] = set()


async def _ensure_worker(client: MedullaClient, worker: str) -> None:
    """A claim is admitted only for a worker the registry holds live (R604.3)."""
    if worker in _REGISTERED:
        return
    await client._send(
        "WorkerRegister",
        {
            "registration": {
                "worker_id": worker,
                "host_iri": "host:alpha",
                "cores": 8,
                "memory_bytes": 8 * 1024**3,
                "lanes_supported": ["general"],
            }
        },
    )
    _REGISTERED.add(worker)
    # Keep the lease alive for the rest of the module's tests.
    await client._send("WorkerHeartbeat", {"worker_id": worker})


async def _claim(client: MedullaClient, worker: str) -> dict[str, Any] | None:
    await _ensure_worker(client, worker)
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
            parked_before = (await _ingest_depth(client, "ingest"))["ingest"]["dead_letters"]
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
            # Five deliveries exhausted the policy: the task is parked as a dead letter, which
            # the served depth report counts, and no sixth delivery follows.
            parked_after = (await _ingest_depth(client, "ingest"))["ingest"]["dead_letters"]
            assert parked_after == parked_before + 1
            await asyncio.sleep(0.5)
            while (extra := await _claim(client, "ingest-worker-1")) is not None:
                assert extra["task_id"] != task_id
        finally:
            await client._send("SetSchedulerConfig", {"config": current})
    finally:
        await client.close()


async def _with_retry_limits(client: MedullaClient, max_attempts: int) -> dict[str, Any]:
    """Set the ingest retry policy; return the config it replaced."""
    current = await client._send("GetSchedulerConfig", {})
    await client._send(
        "SetSchedulerConfig",
        {
            "config": {
                **current,
                "ingest_retry": {
                    "max_attempts": max_attempts,
                    "base_delay_ms": 20,
                    "max_delay_ms": 200,
                    "jitter_milli": 200,
                },
            }
        },
    )
    return current


# spec: MEDULLA-INGEST-WORKER-R001.12
@pytest.mark.spec("MEDULLA-INGEST-WORKER-R001.12")
@pytest.mark.asyncio
async def test_lapsed_lease_redelivers_then_exhausts() -> None:
    client = await _client()
    current = await _with_retry_limits(client, 2)
    try:
        parked_before = (await _ingest_depth(client, "ingest"))["ingest"]["dead_letters"]
        receipt = await client._send(
            "IngestEnqueue", {"envelope": _envelope(f"job-{uuid.uuid4().hex[:10]}")}
        )
        task_id = receipt["task_id"]
        worker = "ingest-worker-1"
        await _ensure_worker(client, worker)
        # Claim with the shortest lease and never renew: each lapse redelivers the task and
        # counts as an attempt, the claim after max_attempts dead-letters it instead.
        deliveries = 0
        deadline = time.monotonic() + 30.0
        while time.monotonic() < deadline and deliveries < 2:
            claimed = await client._send(
                "TaskQueueClaim",
                {"worker_id": worker, "grant_id": "grant-1", "lease_ms": 1_000},
            )
            if claimed is None:
                await asyncio.sleep(0.1)
            elif claimed["task_id"] == task_id:
                deliveries += 1
                assert claimed["attempt"] == deliveries
                await asyncio.sleep(1.2)
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
        assert deliveries == 2
        # The third claim finds the task past its attempts: it is parked, never delivered.
        await asyncio.sleep(0.2)
        while (extra := await _claim(client, worker)) is not None:
            assert extra["task_id"] != task_id
            await client._send(
                "TaskQueueAck",
                {
                    "task_id": extra["task_id"],
                    "worker_id": worker,
                    "outcome": "Succeeded",
                    "detail": "",
                },
            )
        parked_after = (await _ingest_depth(client, "ingest"))["ingest"]["dead_letters"]
        assert parked_after == parked_before + 1
    finally:
        await client._send("SetSchedulerConfig", {"config": current})
        await client.close()


# spec: MEDULLA-INGEST-WORKER-R001.12
@pytest.mark.spec("MEDULLA-INGEST-WORKER-R001.12")
@pytest.mark.asyncio
async def test_renewed_lease_is_not_redelivered() -> None:
    client = await _client()
    current = await _with_retry_limits(client, 2)
    try:
        receipt = await client._send(
            "IngestEnqueue", {"envelope": _envelope(f"job-{uuid.uuid4().hex[:10]}")}
        )
        task_id = receipt["task_id"]
        worker = "ingest-worker-1"
        await _ensure_worker(client, worker)
        deadline = time.monotonic() + 10.0
        claimed: dict[str, Any] | None = None
        while time.monotonic() < deadline and claimed is None:
            candidate = await client._send(
                "TaskQueueClaim",
                {"worker_id": worker, "grant_id": "grant-1", "lease_ms": 1_000},
            )
            if candidate is None:
                await asyncio.sleep(0.1)
            elif candidate["task_id"] == task_id:
                claimed = candidate
            else:
                await client._send(
                    "TaskQueueAck",
                    {
                        "task_id": candidate["task_id"],
                        "worker_id": worker,
                        "outcome": "Succeeded",
                        "detail": "",
                    },
                )
        assert claimed is not None
        # Renewing every third of the lease keeps it: three renewals outlive two leases.
        for _ in range(3):
            await asyncio.sleep(0.33)
            await client._send(
                "TaskQueueRenew",
                {"task_id": task_id, "worker_id": worker, "lease_ms": 1_000},
            )
        while (extra := await _claim(client, worker)) is not None:
            assert extra["task_id"] != task_id
            await client._send(
                "TaskQueueAck",
                {
                    "task_id": extra["task_id"],
                    "worker_id": worker,
                    "outcome": "Succeeded",
                    "detail": "",
                },
            )
        acked = await client._send(
            "TaskQueueAck",
            {
                "task_id": task_id,
                "worker_id": worker,
                "outcome": "Succeeded",
                "detail": "",
            },
        )
        assert acked["task_id"] == task_id
    finally:
        await client._send("SetSchedulerConfig", {"config": current})
        await client.close()


async def _ingest_depth(client: MedullaClient, queue: str | None = None) -> dict[str, Any]:
    report = await client._send("IngestQueueDepth", {"queue": queue})
    return {row["queue"]: row for row in report["queues"]}


# spec: MEDULLA-INGEST-WORKER-R001.11
@pytest.mark.spec("MEDULLA-INGEST-WORKER-R001.11")
@pytest.mark.asyncio
async def test_full_queue_refuses_and_depth_is_unchanged() -> None:
    client = await _client()
    try:
        before_all = await _ingest_depth(client)
        assert list(before_all) == ["ingest", "ingest-hydration"]
        assert list(await _ingest_depth(client, "ingest-hydration")) == ["ingest-hydration"]
        with pytest.raises(Exception, match="INVALID_ARGUMENT"):
            await _ingest_depth(client, "not-an-ingest-queue")

        current = await client._send("GetSchedulerConfig", {})
        limit = before_all["ingest"]["depth"] + 3
        await client._send(
            "SetSchedulerConfig", {"config": {**current, "max_queue_depth": limit}}
        )
        try:
            for _ in range(3):
                await client._send(
                    "IngestEnqueue", {"envelope": _envelope(f"job-{uuid.uuid4().hex[:10]}")}
                )
            full = await _ingest_depth(client)
            assert full["ingest"]["depth"] == limit
            with pytest.raises(Exception, match="INGEST_QUEUE_FULL") as refusal:
                await client._send(
                    "IngestEnqueue", {"envelope": _envelope(f"job-{uuid.uuid4().hex[:10]}")}
                )
            assert f"depth {limit} is at limit {limit}" in str(refusal.value)
            after = await _ingest_depth(client)
            assert after["ingest"]["depth"] == full["ingest"]["depth"]
            assert after["ingest-hydration"]["depth"] == full["ingest-hydration"]["depth"]
        finally:
            await client._send("SetSchedulerConfig", {"config": current})
    finally:
        await client.close()


# spec: MEDULLA-INGEST-WORKER-R001.10
@pytest.mark.spec("MEDULLA-INGEST-WORKER-R001.10")
@pytest.mark.asyncio
async def test_hydration_kinds_use_hydration_queue() -> None:
    client = await _client()
    try:
        queues: dict[str, str] = {}
        for kind in (
            "document",
            "codebase",
            "skill_workflows",
            "capability_hydration",
            "extraction",
        ):
            receipt = await client._send(
                "IngestEnqueue",
                {"envelope": _envelope(f"job-{uuid.uuid4().hex[:10]}", kind=kind)},
            )
            queues[kind] = receipt["queue"]
        assert queues == {
            "document": "ingest",
            "codebase": "ingest",
            "skill_workflows": "ingest-hydration",
            "capability_hydration": "ingest-hydration",
            "extraction": "ingest",
        }
    finally:
        await client.close()


# spec: MEDULLA-INGEST-WORKER-R001.10
@pytest.mark.spec("MEDULLA-INGEST-WORKER-R001.10")
@pytest.mark.asyncio
async def test_one_worker_pool_still_claims_general_work() -> None:
    client = await _client()
    try:
        current = await client._send("GetSchedulerConfig", {})
        policy = {**current, "total_workers": 8, "lane_floors": {"general": 1, "hydration": 1}}
        await client._send("SetSchedulerConfig", {"config": policy})
        try:
            stem = uuid.uuid4().hex[:8]

            def registration(worker: str, lanes: list[str]) -> dict[str, Any]:
                return {
                    "worker_id": worker,
                    "host_iri": "host:alpha",
                    "cores": 8,
                    "memory_bytes": 8 * 1024**3,
                    "lanes_supported": lanes,
                }

            # One worker: the pool is granted the general lane.
            general = f"general-{stem}"
            await client._send(
                "WorkerRegister", {"registration": registration(general, ["general"])}
            )
            first = await client._send(
                "AdmitWorkerTask", {"worker_id": general, "requested_max": 1}
            )
            assert first["Granted"]["reserved_lane"] == "general"

            # A second worker that runs hydration gets the hydration reserve; the first
            # worker still claims general work.
            hydration = f"hydration-{stem}"
            await client._send(
                "WorkerRegister", {"registration": registration(hydration, ["hydration"])}
            )
            second = await client._send(
                "AdmitWorkerTask", {"worker_id": hydration, "requested_max": 1}
            )
            assert second["Granted"]["reserved_lane"] == "hydration"
            again = await client._send(
                "AdmitWorkerTask", {"worker_id": general, "requested_max": 1}
            )
            assert again["Granted"]["reserved_lane"] == "general"
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
