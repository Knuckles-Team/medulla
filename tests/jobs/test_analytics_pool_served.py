"""The served colocated analytics pool runs a job at the configured size."""

import time
import uuid

import pytest


def _state_name(state) -> str:
    """`JobState` is externally tagged: a bare string or a one-key mapping."""
    if isinstance(state, str):
        return state
    assert isinstance(state, dict) and len(state) == 1, f"unexpected state: {state!r}"
    return next(iter(state))


@pytest.mark.spec("MEDULLA-PLACEMENT-R002.14")
def test_job_runs_with_configured_pool(clean_graph):
    # The served engine seeds the colocated pool once from the scheduler
    # configuration (default one worker, at most 32). A job that needs no
    # capability is claimed by that pool and runs to completion.
    graph = f"analytics-pool-{uuid.uuid4().hex[:8]}"
    clean_graph.tenants.create(graph)
    kind = {
        "MineAssociate": {
            "transactions": [["pool label a", "pool label b"]],
            "min_support": 0.1,
            "min_confidence": 0.5,
            "algorithm": "fpgrowth",
        }
    }
    job = clean_graph.jobs.submit(graph, kind)
    job_id = job["job_id"]
    assert job_id
    state_name = _state_name(job["state"])
    for _ in range(100):
        state_name = _state_name(clean_graph.jobs.status(job_id)["state"])
        if state_name in ("Succeeded", "Failed", "Cancelled"):
            break
        time.sleep(0.2)
    assert state_name == "Succeeded", f"pool did not run the job: {state_name}"
