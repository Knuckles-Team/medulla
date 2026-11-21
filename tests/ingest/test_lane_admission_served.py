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
from conftest import request_context

from medulla.client import MedullaClient

pytestmark = [pytest.mark.spec("MEDULLA-INGEST-WORKER-R604.3")]


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
