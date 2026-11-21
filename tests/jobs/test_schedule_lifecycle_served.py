"""Served owned-schedule lifecycle: runs, pauses, skips and reports.

spec: MEDULLA-PLACEMENT-R751.759

One engine serves schedule create -> `Schedule` node -> executor tick task ->
`TickLedger::record` -> job submit, with the budget, pause and overlap checks before the
submit. Every outcome is observed through the served tick-history read
(`ScheduleTicks`, MEDULLA-PLACEMENT-R751.758): a fired tick links its work item; a
schedule that spent its budget, a paused schedule and a schedule whose previous run is
still active each record a skipped tick with its reason and submit nothing.
"""

import time
import uuid

import pytest

GRAPH = "__commons__"
EVERY_MS = 2_000
PERIOD_S = 12
REMOTE_CAPABILITY = "association-remote-worker"


def _ticks(client, name: str) -> list[dict]:
    return client.jobs.schedule_ticks(GRAPH, name)["ticks"]


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


@pytest.mark.spec("MEDULLA-PLACEMENT-R751.759")
def test_owned_schedule_runs_pauses_skips_and_reports(clean_graph):
    suffix = uuid.uuid4().hex[:8]
    runs = f"life-runs-{suffix}"
    budget = f"life-budget-{suffix}"
    remote = f"life-remote-{suffix}"
    now_ms = int(time.time() * 1000)
    interval = {"interval": {"every_ms": EVERY_MS, "start_ms": now_ms}}

    created = clean_graph.jobs.schedule_create(
        GRAPH, runs, interval, "association.refresh"
    )
    assert created["state"] == "enabled"
    clean_graph.jobs.schedule_create(
        GRAPH,
        budget,
        interval,
        "association.refresh",
        max_run_s=1,
        max_tokens=1_000_000,
        period_s=PERIOD_S,
    )
    # The remote kind needs a worker capability no worker holds: its run stays active.
    clean_graph.jobs.schedule_create(GRAPH, remote, interval, "association.remote")
    claimed = None
    try:
        # Runs: the first due window fires and links the work item it submitted.
        fired = _wait_for(
            lambda: [t for t in _ticks(clean_graph, runs) if t["outcome"] == "fired"],
            30,
        )
        assert fired, "the first due window fires"
        assert fired[0]["work_item_id"], "a fired tick links its work item"
        assert fired[0]["reason"] == ""

        # Budget: a run charged one second spends the whole budget, so a later window of
        # the same period records the budget skip and submits nothing.
        exhausted = _wait_for(
            lambda: [
                t
                for t in _ticks(clean_graph, budget)
                if t["reason"] == "budget_exhausted"
            ],
            40,
        )
        assert exhausted, "a schedule that spent its budget skips its next window"
        budget_ticks = _ticks(clean_graph, budget)
        assert [t["outcome"] for t in budget_ticks if t["outcome"] == "fired"], (
            "the first window had budget and fired"
        )
        assert all(
            t["work_item_id"] is None
            for t in budget_ticks
            if t["outcome"] == "skipped"
        )

        # Overlap: later windows skip while the first run is still active, and exactly
        # one work item was ever submitted.
        overlap = _wait_for(
            lambda: [
                t for t in _ticks(clean_graph, remote) if t["reason"] == "overlap"
            ],
            30,
        )
        assert overlap, "a window due during an active run records overlap"
        remote_ticks = _ticks(clean_graph, remote)
        remote_fired = [t for t in remote_ticks if t["outcome"] == "fired"]
        assert len(remote_fired) == 1 and remote_fired[0]["work_item_id"]
        assert all(
            t["work_item_id"] is None
            for t in remote_ticks
            if t["outcome"] == "skipped"
        )
        claimed, worker = _claim_own(
            clean_graph, f"life-slot-{suffix}", remote_fired[0]["work_item_id"]
        )

        # Pause: each window that comes due while paused is skipped, none fires late.
        paused = clean_graph.jobs.schedule_pause(GRAPH, runs)
        assert paused["state"] == "paused"
        paused_at_ms = int(time.time() * 1000)
        skipped = _wait_for(
            lambda: [
                t
                for t in _ticks(clean_graph, runs)
                if t["reason"] == "paused" and t["window_start_ms"] >= paused_at_ms
            ],
            30,
        )
        assert skipped, "a window that comes due while paused is skipped"
        assert all(t["work_item_id"] is None for t in skipped)

        # Resume: the schedule fires again from its next window, without backfill.
        resumed = clean_graph.jobs.schedule_resume(GRAPH, runs)
        assert resumed["state"] == "enabled"
        resumed_ms = resumed["resumed_ms"]
        again = _wait_for(
            lambda: [
                t
                for t in _ticks(clean_graph, runs)
                if t["outcome"] == "fired" and t["window_start_ms"] >= resumed_ms
            ],
            30,
        )
        assert again, "the schedule fires again after resume"

        # The served history reports every outcome in time order with the budget used.
        history = clean_graph.jobs.schedule_ticks(GRAPH, runs)
        assert history["schedule"] == runs
        windows = [t["window_start_ms"] for t in history["ticks"]]
        assert windows == sorted(windows) and len(windows) == len(set(windows))
        assert {t["outcome"] for t in history["ticks"]} >= {"fired", "skipped"}
        assert all(
            t["used_run_s"] >= 0 and t["used_tokens"] >= 0 for t in history["ticks"]
        )
        for tick in history["ticks"]:
            if tick["window_start_ms"] < resumed_ms and tick["reason"] == "paused":
                assert tick["outcome"] == "skipped"
    finally:
        if claimed is not None:
            _release(clean_graph, worker, claimed)
        for name in (runs, budget, remote):
            clean_graph.jobs.schedule_delete(GRAPH, name)
