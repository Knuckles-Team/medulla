"""Served skip-never-queue: overlapping and missed windows skip without queueing.

spec: MEDULLA-PLACEMENT-R751.757

A schedule has at most one active run. A window that comes due while the previous run is
still active records `skipped` with reason `overlap`; a window that came due while no
leader
held the lease records `skipped` with reason `missed`. Neither submits a job, so the job
store never holds a backlog of scheduled work.
"""

import time
import uuid

import pytest

GRAPH = "__commons__"
LEASE = "leader_lease:schedules"
EVERY_MS = 2_000
REMOTE_CAPABILITY = "association-remote-worker"


def _ticks(client, name: str) -> list[dict]:
    ticks = []
    for row in client.nodes.list():
        if row[0].startswith("schedule-tick:"):
            props = client.nodes.properties(row[0])
            if props.get("schedule") == name:
                ticks.append(props)
    return sorted(ticks, key=lambda props: props["window_start_ms"])


def _wait_for(predicate, timeout_s: float):
    deadline = time.time() + timeout_s
    while time.time() < deadline:
        found = predicate()
        if found:
            return found
        time.sleep(0.5)
    return predicate()


def _release(client, slot: str, claimed: dict) -> None:
    """Cancel a claimed job and confirm it as its worker, freeing its active slot."""
    job_id = claimed["job"]["job_id"]
    client.jobs.cancel(job_id)
    client.jobs.worker_cancel(job_id, slot, claimed["lease"]["epoch"])


def _claim_own(client, slot: str, job_id: str) -> tuple[dict, str]:
    """Claim until this test's job comes up, releasing capability-free leftovers."""
    for attempt in range(16):
        worker = f"{slot}-{attempt}"
        claimed = client.jobs.worker_claim(worker, [REMOTE_CAPABILITY])
        assert claimed is not None
        if claimed["job"]["job_id"] == job_id:
            return claimed, worker
        _release(client, worker, claimed)
    raise AssertionError(f"{job_id} was not claimed within 16 claims")


def _lease(now_ms: int, revision: int, expires_ms: int) -> dict:
    return {
        "op": "upsert_node",
        "id": LEASE,
        "properties": {
            "type": "LeaderLease",
            "state": "held",
            "holder": "rival-engine",
            "fence": 1_000_000,
            "revision": revision,
            "expires_ms": expires_ms,
        },
    }


@pytest.mark.spec("MEDULLA-PLACEMENT-R751.757")
def test_overlap_and_missed_windows_skip_without_queueing(clean_graph):
    suffix = uuid.uuid4().hex[:8]
    remote = f"skip-remote-{suffix}"
    local = f"skip-local-{suffix}"
    now_ms = int(time.time() * 1000)
    interval = {"Interval": {"every_ms": EVERY_MS, "start_ms": now_ms}}
    # The remote kind needs a worker capability no worker holds: its run stays active.
    clean_graph.jobs.schedule_create(GRAPH, remote, interval, "association.remote")
    clean_graph.jobs.schedule_create(GRAPH, local, interval, "association.refresh")
    try:
        # Overlap: later windows skip while the first run is still active.
        overlap = _wait_for(
            lambda: [
                t for t in _ticks(clean_graph, remote) if t["reason"] == "overlap"
            ],
            30,
        )
        assert len(overlap) >= 1
        ticks = _ticks(clean_graph, remote)
        fired = [t for t in ticks if t["outcome"] == "fired"]
        # Exactly one work item was ever submitted: skipped windows queue nothing.
        assert len(fired) == 1 and fired[0]["job_id"]
        assert all(t["job_id"] == "" for t in ticks if t["outcome"] == "skipped")
        claimed, worker = _claim_own(
            clean_graph, f"skip-slot-{suffix}", fired[0]["job_id"]
        )

        # Missed: a rival holds the lease for several windows; the schedule's windows in
        # that gap are recorded as missed once this engine leads again.
        assert _wait_for(
            lambda: [t for t in _ticks(clean_graph, local) if t["outcome"] == "fired"],
            30,
        )
        rival_until = int(time.time() * 1000) + 600_000
        clean_graph.lifecycle.batch_update([_lease(now_ms, 1_000_000, rival_until)])
        time.sleep(1.5)
        gap_start = len(_ticks(clean_graph, local))
        last_before = _ticks(clean_graph, local)[-1]["window_start_ms"]
        time.sleep(3 * EVERY_MS / 1000 + 1)
        assert len(_ticks(clean_graph, local)) == gap_start, "no leader, no ticks"
        clean_graph.lifecycle.batch_update([_lease(now_ms, 1_000_001, 1)])
        missed = _wait_for(
            lambda: [t for t in _ticks(clean_graph, local) if t["reason"] == "missed"],
            30,
        )
        assert len(missed) >= 2, "each window of the gap records a missed tick"
        for tick in missed:
            assert tick["outcome"] == "skipped"
            assert tick["job_id"] == ""
            assert tick["window_start_ms"] > last_before
        # The gap is never backfilled: after it the schedule fires again.
        last_missed = max(t["window_start_ms"] for t in missed)
        assert _wait_for(
            lambda: [
                t
                for t in _ticks(clean_graph, local)
                if t["outcome"] == "fired" and t["window_start_ms"] > last_missed
            ],
            30,
        )
        _release(clean_graph, worker, claimed)
    finally:
        clean_graph.jobs.schedule_delete(GRAPH, remote)
        clean_graph.jobs.schedule_delete(GRAPH, local)
