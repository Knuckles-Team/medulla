"""Served claim pacing: background claims pause while a foreground job is queued."""

import uuid

import pytest

from medulla.client import EngineResponseError

FOREGROUND_PRIORITY = 90
BACKGROUND_PRIORITY = 0


def _graph(client) -> str:
    """A job is anchored to an existing graph of the caller."""
    graph = f"claim-pacing-{uuid.uuid4().hex[:8]}"
    client.tenants.create(graph)
    return graph


def _kind() -> dict:
    return {
        "MineAssociate": {
            "transactions": [["pacing label a", "pacing label b"]],
            "min_support": 0.1,
            "min_confidence": 0.5,
            "algorithm": "fpgrowth",
        }
    }


def _release(client, slot: str, claimed: dict) -> None:
    """Cancel a claimed job and confirm it as its worker.

    A cancel request alone leaves a leased job active, holding one of the
    tenant's active-job slots until its lease runs out.
    """

    job_id = claimed["job"]["job_id"]
    client.jobs.cancel(job_id)
    client.jobs.worker_cancel(job_id, slot, claimed["lease"]["epoch"])


def _claim_own(
    client, slot: str, capabilities: list[str], job_id: str
) -> tuple[dict, str]:
    """Claim until this test's job comes up.

    A worker's claim also matches capability-free jobs, so one an earlier test
    of this shared engine left queued can come up first, at either priority;
    each such job is released until `job_id` is claimed. Every attempt claims
    from its own worker slot: a slot that already holds a lease is answered
    with that lease again. Returns the claim and the slot that holds it.
    """

    for attempt in range(16):
        worker = f"{slot}-{attempt}"
        claimed = client.jobs.worker_claim(worker, capabilities)
        assert claimed is not None
        if claimed["job"]["job_id"] == job_id:
            return claimed, worker
        _release(client, worker, claimed)
    raise AssertionError(f"{job_id} was not claimed within 16 claims")


@pytest.mark.spec("MEDULLA-PLACEMENT-R750.753")
def test_background_claims_pause_under_foreground_load(clean_graph):
    nonce = uuid.uuid4().hex
    graph = _graph(clean_graph)
    foreground_cap = f"pacing-foreground-{nonce}"
    background_cap = f"pacing-background-{nonce}"
    # Unique capabilities keep the server's own analytics executor from
    # claiming either job.
    foreground = clean_graph.jobs.submit(
        graph,
        _kind(),
        priority=FOREGROUND_PRIORITY,
        required_capabilities=[foreground_cap],
    )
    background = clean_graph.jobs.submit(
        graph,
        _kind(),
        priority=BACKGROUND_PRIORITY,
        required_capabilities=[background_cap],
    )

    # A foreground job is queued, so the background claim is paced and the job
    # stays queued.
    with pytest.raises((EngineResponseError, RuntimeError), match="CLAIM_PACED"):
        clean_graph.jobs.worker_claim(f"pacing-slot-{nonce}", [background_cap])

    # The foreground job is not throttled.
    claimed, worker = _claim_own(
        clean_graph,
        f"pacing-foreground-slot-{nonce}",
        [foreground_cap],
        foreground["job_id"],
    )
    # Release the proven foreground job so it holds no tenant active slot.
    _release(clean_graph, worker, claimed)

    # No foreground job is queued any more, so the background claim proceeds.
    claimed, worker = _claim_own(
        clean_graph,
        f"pacing-background-slot-{nonce}",
        [background_cap],
        background["job_id"],
    )
    _release(clean_graph, worker, claimed)


@pytest.mark.spec("MEDULLA-PLACEMENT-R750.753")
def test_priority_above_100_is_refused_at_enqueue(clean_graph):
    with pytest.raises(
        (EngineResponseError, RuntimeError), match="PRIORITY_OUT_OF_RANGE"
    ):
        clean_graph.jobs.submit(_graph(clean_graph), _kind(), priority=101)
