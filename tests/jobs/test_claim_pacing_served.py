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
    claimed = clean_graph.jobs.worker_claim(f"pacing-slot-{nonce}", [foreground_cap])
    assert claimed is not None
    assert claimed["job"]["job_id"] == foreground["job_id"]

    # No foreground job is queued any more, so the background claim proceeds.
    # A capability-free job an earlier test of this engine left queued is also
    # compatible with the claim; it is cancelled until this test's job comes up.
    for _ in range(16):
        claimed = clean_graph.jobs.worker_claim(
            f"pacing-slot-{nonce}", [background_cap]
        )
        assert claimed is not None
        if claimed["job"]["job_id"] == background["job_id"]:
            break
        clean_graph.jobs.cancel(claimed["job"]["job_id"])
    assert claimed["job"]["job_id"] == background["job_id"]


@pytest.mark.spec("MEDULLA-PLACEMENT-R750.753")
def test_priority_above_100_is_refused_at_enqueue(clean_graph):
    with pytest.raises(
        (EngineResponseError, RuntimeError), match="PRIORITY_OUT_OF_RANGE"
    ):
        clean_graph.jobs.submit(_graph(clean_graph), _kind(), priority=101)
