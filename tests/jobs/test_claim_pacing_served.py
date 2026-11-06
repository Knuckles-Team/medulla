"""Served claim pacing: background claims pause while a foreground job is queued."""

import uuid

import pytest

from medulla.client import EngineResponseError

FOREGROUND_PRIORITY = 90
BACKGROUND_PRIORITY = 0


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
    foreground_cap = f"pacing-foreground-{nonce}"
    background_cap = f"pacing-background-{nonce}"
    # Unique capabilities keep the server's own analytics executor from
    # claiming either job.
    foreground = clean_graph.jobs.submit(
        "claim-pacing",
        _kind(),
        priority=FOREGROUND_PRIORITY,
        required_capabilities=[foreground_cap],
    )
    background = clean_graph.jobs.submit(
        "claim-pacing",
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
    claimed = clean_graph.jobs.worker_claim(f"pacing-slot-{nonce}", [background_cap])
    assert claimed is not None
    assert claimed["job"]["job_id"] == background["job_id"]


@pytest.mark.spec("MEDULLA-PLACEMENT-R750.753")
def test_priority_above_100_is_refused_at_enqueue(clean_graph):
    with pytest.raises(
        (EngineResponseError, RuntimeError), match="PRIORITY_OUT_OF_RANGE"
    ):
        clean_graph.jobs.submit("claim-pacing", _kind(), priority=101)
