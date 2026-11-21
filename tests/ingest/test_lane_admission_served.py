"""Served claim admission: MEDULLA-INGEST-WORKER-R604.3.

`TaskQueueClaim` runs the one reserved-worker admission rule before it leases anything: a
worker the registry does not hold live is refused `WORKER_UNKNOWN`, and a registered worker is
admitted whatever lane it serves. The runner never asks first; the claim alone decides.
"""

from __future__ import annotations

import os
import uuid
from typing import Any

import pytest
from conftest import request_context, tenant_request_context

from medulla.client import MedullaClient

pytestmark = [pytest.mark.spec("MEDULLA-INGEST-WORKER-R604.3")]

_R604_4 = pytest.mark.spec("MEDULLA-INGEST-WORKER-R604.4")


def _registration(worker: str, lane: str) -> dict[str, Any]:
    return {
        "worker_id": worker,
        "host_iri": "host:alpha",
        "cores": 8,
        "memory_bytes": 8 * 1024**3,
        "lanes_supported": [lane],
    }


async def _claim(client: MedullaClient, worker: str) -> Any:
    return await client._send(
        "TaskQueueClaim",
        {"worker_id": worker, "grant_id": "grant-1", "lease_ms": 60_000},
    )


# spec: MEDULLA-INGEST-WORKER-R604.3
@pytest.mark.asyncio
async def test_claim_enforces_admission() -> None:
    client = await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
    )
    try:
        current = await client._send("GetSchedulerConfig", {})
        policy = {**current, "total_workers": 8, "lane_floors": {"general": 1, "hydration": 1}}
        await client._send("SetSchedulerConfig", {"config": policy})
        try:
            stem = uuid.uuid4().hex[:8]
            general, hydration = f"general-{stem}", f"hydration-{stem}"

            # A worker that never registered is refused by the claim alone.
            with pytest.raises(Exception, match="WORKER_UNKNOWN"):
                await _claim(client, general)

            # Two registered workers over two lanes are admitted: the claim answers a task or
            # nil, never a refusal.
            for worker, lane in ((general, "general"), (hydration, "hydration")):
                await client._send(
                    "WorkerRegister", {"registration": _registration(worker, lane)}
                )
            for worker in (general, hydration):
                claimed = await _claim(client, worker)
                assert claimed is None or "task_id" in claimed
        finally:
            await client._send("SetSchedulerConfig", {"config": current})
    finally:
        await client.close()


# spec: MEDULLA-INGEST-WORKER-R604.4
@_R604_4
@pytest.mark.asyncio
async def test_reserve_refusal_has_code_and_retry_hint() -> None:
    stem = uuid.uuid4().hex[:8]
    tenant = f"reserve-{stem}"
    client = await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=tenant_request_context(tenant),
    )
    try:
        current = await client._send("GetSchedulerConfig", {})
        # One worker in the pool and a floor of one for the general lane: any grant starves the
        # reserve, so only a lane with queued work and no running worker may claim.
        policy = {
            **current,
            "total_workers": 1,
            "lane_floors": {"general": 1},
            "admit_retry_ms": 700,
        }
        await client._send("SetSchedulerConfig", {"config": policy})
        try:
            worker = f"general-{stem}"
            await client._send(
                "WorkerRegister", {"registration": _registration(worker, "general")}
            )
            before = await client._send("IngestQueueDepth", {"queue": "ingest"})
            with pytest.raises(Exception, match="ADMIT_HELD_FOR_RESERVE") as refusal:
                await _claim(client, worker)
            assert "general" in str(refusal.value)
            assert "retry_after_ms 700" in str(refusal.value)
            # The refusal wrote nothing.
            after = await client._send("IngestQueueDepth", {"queue": "ingest"})
            assert after == before

            # The exception: a lane with queued work and no running worker is relieved.
            await client._send(
                "IngestEnqueue",
                {
                    "envelope": {
                        "job_id": f"job-{stem}",
                        "tenant": tenant,
                        "corpus_key": "corpus-a",
                        "kind": "document",
                        "target": "doc://a",
                        "attempt": 1,
                    }
                },
            )
            claimed = await _claim(client, worker)
            assert claimed is not None and "task_id" in claimed
        finally:
            await client._send("SetSchedulerConfig", {"config": current})
    finally:
        await client.close()
