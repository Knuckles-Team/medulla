"""Served pause and resume: paused windows are skipped and resume does not backfill.

spec: MEDULLA-PLACEMENT-R751.756

A schedule holds an `enabled` or `paused` state (the `ScheduleMachine`). Pause and
resume are idempotent. Each window that comes due while paused records a skipped tick
with reason
`paused`; after resume only windows from the resume instant on fire.
"""

import time
import uuid

import pytest

GRAPH = "__commons__"
EVERY_MS = 2_000


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


@pytest.mark.spec("MEDULLA-PLACEMENT-R751.756")
def test_paused_windows_are_skipped_and_resume_does_not_backfill(clean_graph):
    name = f"pause-{uuid.uuid4().hex[:8]}"
    now_ms = int(time.time() * 1000)
    created = clean_graph.jobs.schedule_create(
        GRAPH,
        name,
        {"Interval": {"every_ms": EVERY_MS, "start_ms": now_ms}},
        "association.refresh",
    )
    assert created["state"] == "enabled"
    try:
        before_pause_ms = int(time.time() * 1000)
        paused = clean_graph.jobs.schedule_pause(GRAPH, name)
        assert paused["state"] == "paused"
        # Pausing a paused schedule changes nothing.
        assert clean_graph.jobs.schedule_pause(GRAPH, name) == paused
        assert clean_graph.jobs.schedule_get(GRAPH, name) == paused

        floor_ms = before_pause_ms + 1_000

        def paused_ticks():
            return [
                t
                for t in _ticks(clean_graph, name)
                if t["window_start_ms"] >= floor_ms and t["reason"] == "paused"
            ]

        assert (
            len(_wait_for(lambda: len(paused_ticks()) >= 2 and paused_ticks(), 30)) >= 2
        )
        # Nothing fired for a window that came due while paused.
        fired = [
            t
            for t in _ticks(clean_graph, name)
            if t["outcome"] == "fired" and t["window_start_ms"] >= floor_ms
        ]
        assert fired == []

        before_resume_ms = int(time.time() * 1000)
        resumed = clean_graph.jobs.schedule_resume(GRAPH, name)
        assert resumed["state"] == "enabled"
        assert resumed["resumed_ms"] >= before_resume_ms
        # Resuming an enabled schedule changes nothing.
        assert clean_graph.jobs.schedule_resume(GRAPH, name) == resumed
        resumed_ms = resumed["resumed_ms"]

        after = _wait_for(
            lambda: [
                t
                for t in _ticks(clean_graph, name)
                if t["outcome"] == "fired" and t["window_start_ms"] >= resumed_ms
            ],
            30,
        )
        assert after, "the schedule fires again from its next window"

        ticks = [
            t for t in _ticks(clean_graph, name) if t["window_start_ms"] >= floor_ms
        ]
        for tick in ticks:
            if tick["window_start_ms"] < resumed_ms:
                # A window that passed while paused is never fired late.
                assert tick["outcome"] == "skipped"
                assert tick["reason"] == "paused"
                assert tick["job_id"] == ""
            elif tick["outcome"] == "fired":
                assert tick["job_id"]
        windows = [t["window_start_ms"] for t in ticks]
        assert len(windows) == len(set(windows))
        assert all((w - now_ms) % EVERY_MS == 0 for w in windows)
    finally:
        clean_graph.jobs.schedule_delete(GRAPH, name)
