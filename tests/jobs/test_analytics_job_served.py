"""Served proof of the analytics job machine (MEDULLA-CQ-ORCH-R811.4): a cancel during a run
ends in Cancelled through CancelRequested, and a Failed job never revives."""

import uuid

import pytest

from medulla.client import TransitionRefusedError


def _graph(client) -> str:
    """A job is anchored to an existing graph of the caller."""
    graph = f"analytics-job-{uuid.uuid4().hex[:8]}"
    client.tenants.create(graph)
    return graph


def _kind() -> dict:
    return {
        "MineAssociate": {
            "transactions": [["machine label a", "machine label b"]],
            "min_support": 0.1,
            "min_confidence": 0.5,
            "algorithm": "fpgrowth",
        }
    }


def _state_name(job: dict) -> str:
    """The variant name of the externally tagged job state."""
    state = job["state"]
    return state if isinstance(state, str) else next(iter(state))


@pytest.mark.spec("MEDULLA-CQ-ORCH-R811.4")
def test_cancel_during_run_ends_cancelled_and_failed_never_revives(clean_graph):
    nonce = uuid.uuid4().hex
    graph = _graph(clean_graph)
    # Unique capabilities keep the server's own analytics executor from claiming either job.
    cancel_cap = f"machine-cancel-{nonce}"
    fail_cap = f"machine-fail-{nonce}"

    # A cancel during a run: Running, then CancelRequested, then the holder confirms.
    cancelled = clean_graph.jobs.submit(
        graph, _kind(), required_capabilities=[cancel_cap]
    )
    slot = f"machine-slot-{nonce}"
    claimed = clean_graph.jobs.worker_claim(slot, [cancel_cap])
    assert claimed is not None and claimed["job"]["job_id"] == cancelled["job_id"]
    assert _state_name(claimed["job"]) == "Running"
    assert claimed["job"]["state"]["Running"]["lease"]["worker_ref"] == claimed["lease"]["worker_ref"]
    requested = clean_graph.jobs.cancel(cancelled["job_id"])
    assert _state_name(requested) == "CancelRequested"
    done = clean_graph.jobs.worker_cancel(
        cancelled["job_id"], slot, claimed["lease"]["epoch"]
    )
    assert _state_name(done) == "Cancelled"

    # A failed job (one attempt) never revives: a resume is refused by the machine.
    failing = clean_graph.jobs.submit(
        graph, _kind(), required_capabilities=[fail_cap], max_attempts=1
    )
    fail_slot = f"machine-fail-slot-{nonce}"
    claim = clean_graph.jobs.worker_claim(fail_slot, [fail_cap])
    assert claim is not None and claim["job"]["job_id"] == failing["job_id"]
    failed = clean_graph.jobs.worker_fail(
        failing["job_id"], fail_slot, claim["lease"]["epoch"], "kernel_failure"
    )
    assert _state_name(failed) == "Failed"
    with pytest.raises(TransitionRefusedError) as refused:
        clean_graph.jobs.resume(failing["job_id"])
    assert refused.value.code == "TRANSITION_REFUSED"
    assert refused.value.machine == "AnalyticsJobMachine"
    assert refused.value.from_state == "failed"
    assert refused.value.event == "Claim"
    assert refused.value.reason == "terminal_state"
    assert _state_name(clean_graph.jobs.status(failing["job_id"])) == "Failed"
