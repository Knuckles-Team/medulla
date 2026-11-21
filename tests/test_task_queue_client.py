"""Served proof for MEDULLA-DURABLE-KERNEL-R073: the task queue Python client.

One enqueued task is claimed under a grant, renewed, acknowledged `Succeeded`,
and a second claim finds nothing ready. Every step goes through
`client.task_queue`, the typed client over the served `TaskQueue*` methods.
"""

import uuid

import pytest

from medulla.generated import models


@pytest.mark.spec("MEDULLA-DURABLE-KERNEL-R073")
def test_enqueue_claim_ack_round_trip_against_a_served_engine(clean_graph) -> None:
    queue = f"r073.{uuid.uuid4().hex[:12]}"
    queued = clean_graph.task_queue.enqueue(
        queue=queue,
        kind="scan",
        payload_ref="blob:r073",
        idempotency_key=f"r073-{uuid.uuid4().hex}",
    )
    assert isinstance(queued, models.TaskEnqueued)

    claimed = clean_graph.task_queue.claim(
        worker_id="worker-r073", grant_id="grant-r073", lease_ms=60_000
    )
    assert isinstance(claimed, models.ClaimedTask)
    assert claimed.task_id == queued.task_id
    assert claimed.queue == queue
    assert claimed.kind == "scan"
    assert claimed.payload_ref == "blob:r073"
    assert claimed.attempt == 1

    renewed = clean_graph.task_queue.renew(
        task_id=claimed.task_id, worker_id="worker-r073", lease_ms=120_000
    )
    assert isinstance(renewed, models.TaskRenewed)
    assert renewed.task_id == claimed.task_id
    assert renewed.lease_expires_at_ms > claimed.lease_expires_at_ms

    acked = clean_graph.task_queue.ack(
        task_id=claimed.task_id, worker_id="worker-r073", outcome="Succeeded"
    )
    assert isinstance(acked, models.TaskAcked)
    assert acked.outcome == models.TaskOutcome.SUCCEEDED

    assert (
        clean_graph.task_queue.claim(
            worker_id="worker-r073", grant_id="grant-r073", lease_ms=60_000
        )
        is None
    )
