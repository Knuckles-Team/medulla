"""Served ingest enqueue: MEDULLA-INGEST-WORKER-R001.4 and R001.4.1.

`IngestEnqueue` maps one ingest job onto the reserved served task queue with a
deterministic task id. A duplicate returns the first receipt and writes nothing, a
forged tenant or a credential-bearing target is refused with no task written, and the
task a worker claims carries the receipt's task id.
"""

from __future__ import annotations

import os
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
