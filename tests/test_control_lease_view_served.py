"""MEDULLA-DURABLE-KERNEL-R039.6: a decided lease view names its decider and time."""

from __future__ import annotations

import asyncio
import os
import time
import uuid

from conftest import TEST_TENANT, request_context

from medulla.client import MedullaClient


async def _scenario() -> None:
    client = await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
        timeout=30,
    )
    try:
        now = int(time.time() * 1000)
        lease_id = f"action_approval:{uuid.uuid4().hex[:12]}"
        issued = await client.control_leases.issue(
            tenant=TEST_TENANT,
            lease_id=lease_id,
            kind="action.approval",
            grant={"kind": "deploy"},
            issued_at_ms=now,
            expires_at_ms=now + 600_000,
            hard_expires_at_ms=now + 600_000,
            idempotency_key=f"issue-{lease_id}",
        )
        pending = issued["lease"]
        assert pending["updated_at_ms"] > 0
        assert "decided_by" not in pending
        decided = await client.control_leases.transition(
            tenant=TEST_TENANT,
            lease_id=lease_id,
            expected_revision=pending["revision"],
            to="consumed",
            idempotency_key=f"decide-{lease_id}",
        )
        assert decided["outcome"] == "applied"
        view = await client.control_leases.get(tenant=TEST_TENANT, lease_id=lease_id)
        assert view is not None
        assert view["decided_by"]
        assert view["updated_at_ms"] >= pending["updated_at_ms"]
        assert view["issued_at_ms"] == now
    finally:
        await client.close()


# spec: MEDULLA-DURABLE-KERNEL-R039.6
def test_a_decided_lease_view_names_the_decider_and_the_decision_time():
    asyncio.run(_scenario())
