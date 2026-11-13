"""Served exclusion group: members never run at once and respect the minimum spacing.

spec: MEDULLA-PLACEMENT-R751.760

Two schedules of one exclusion group: the second skips with `group_active` while the
first
runs, with `group_spacing` inside `min_spacing_s` of the first's fired window, and fires
outside it. A refused group name changes nothing.
"""

import time
import uuid

import pytest

from medulla.client import EngineResponseError

GRAPH = "__commons__"
EVERY_MS = 2_000
SPACING_S = 15
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


@pytest.mark.spec("MEDULLA-PLACEMENT-R751.760")
def test_group_members_never_overlap_and_respect_spacing(clean_graph):
    suffix = uuid.uuid4().hex[:8]
    group = f"group-{suffix}"
    first = f"group-first-{suffix}"
    second = f"group-second-{suffix}"
    now_ms = int(time.time() * 1000)

    # An empty or over-long group name is refused and nothing is stored.
    for bad in ("", "g" * 65):
        with pytest.raises(
            (EngineResponseError, RuntimeError), match="INVALID_SCHEDULE"
        ):
            clean_graph.jobs.schedule_create(
                GRAPH,
                f"group-bad-{suffix}",
                {"OneShot": {"at_ms": now_ms + 10**12}},
                "association.refresh",
                exclusion_group=bad,
            )
    with pytest.raises((EngineResponseError, RuntimeError), match="SCHEDULE_NOT_FOUND"):
        clean_graph.jobs.schedule_get(GRAPH, f"group-bad-{suffix}")

    # The first member runs on a remote worker capability nobody holds, so its run stays
    # active; the second member starts one second later with a minimum spacing.
    clean_graph.jobs.schedule_create(
        GRAPH,
        first,
        {"Interval": {"every_ms": EVERY_MS, "start_ms": now_ms}},
        "association.remote",
        exclusion_group=group,
    )
    created = clean_graph.jobs.schedule_create(
        GRAPH,
        second,
        {"Interval": {"every_ms": EVERY_MS, "start_ms": now_ms + 1_000}},
        "association.refresh",
        exclusion_group=group,
        min_spacing_s=SPACING_S,
    )
    assert created["exclusion_group"] == group
    assert created["min_spacing_s"] == SPACING_S
    try:
        active = _wait_for(
            lambda: [
                t for t in _ticks(clean_graph, second) if t["reason"] == "group_active"
            ],
            30,
        )
        assert active, "the second member skips while the first member's run is active"
        assert not [t for t in _ticks(clean_graph, second) if t["outcome"] == "fired"]
        first_fired = [t for t in _ticks(clean_graph, first) if t["outcome"] == "fired"]
        assert len(first_fired) == 1
        fired_ms = first_fired[0]["window_start_ms"]

        # End the first member's run and stop it from firing again; its fired window
        # still
        # holds the group spacing.
        clean_graph.jobs.schedule_pause(GRAPH, first)
        claimed, worker = _claim_own(
            clean_graph, f"group-slot-{suffix}", first_fired[0]["job_id"]
        )
        _release(clean_graph, worker, claimed)

        spaced = _wait_for(
            lambda: [
                t for t in _ticks(clean_graph, second) if t["reason"] == "group_spacing"
            ],
            30,
        )
        assert spaced, "inside the spacing the second member skips"
        assert all(t["window_start_ms"] < fired_ms + SPACING_S * 1_000 for t in spaced)

        # Outside the spacing the second member fires.
        outside = _wait_for(
            lambda: [t for t in _ticks(clean_graph, second) if t["outcome"] == "fired"],
            40,
        )
        assert outside, "past the spacing the second member fires"
        assert outside[0]["window_start_ms"] >= fired_ms + SPACING_S * 1_000
        assert all(
            t["job_id"] == ""
            for t in _ticks(clean_graph, second)
            if t["outcome"] == "skipped"
        )
    finally:
        clean_graph.jobs.schedule_delete(GRAPH, first)
        clean_graph.jobs.schedule_delete(GRAPH, second)
