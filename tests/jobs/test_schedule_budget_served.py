"""Served per-schedule budget: a schedule that spent its budget records a skipped tick.

spec: MEDULLA-PLACEMENT-R751.755

Each schedule carries a budget of run seconds and tokens per budget period. A run is
charged to its schedule when its work item completes; the next window of a
schedule whose
period usage has reached a limit records `skipped` with reason `budget_exhausted` and
submits nothing. The first window of the next budget period fires again.
"""

import time
import uuid

import pytest

GRAPH = "__commons__"
EVERY_MS = 2_000
PERIOD_S = 12


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


@pytest.mark.spec("MEDULLA-PLACEMENT-R751.755")
def test_exhausted_budget_skips_the_tick(clean_graph):
    name = f"budget-{uuid.uuid4().hex[:8]}"
    idle = f"{name}-idle"
    now_ms = int(time.time() * 1000)

    # A schedule without budget fields carries the server's default budget.
    default = clean_graph.jobs.schedule_create(
        GRAPH, idle, {"OneShot": {"at_ms": now_ms + 10**12}}, "association.refresh"
    )
    assert default["max_run_s"] > 0
    assert default["max_tokens"] > 0
    assert default["period_s"] > 0

    created = clean_graph.jobs.schedule_create(
        GRAPH,
        name,
        {"Interval": {"every_ms": EVERY_MS, "start_ms": now_ms}},
        "association.refresh",
        max_run_s=1,
        max_tokens=1_000_000,
        period_s=PERIOD_S,
    )
    assert (created["max_run_s"], created["period_s"]) == (1, PERIOD_S)
    try:
        # The first window fires; its run is charged at least one second, which is the
        # whole budget, so a later window of the same period records the budget skip.
        exhausted = _wait_for(
            lambda: [
                t
                for t in _ticks(clean_graph, name)
                if t["reason"] == "budget_exhausted"
            ],
            40,
        )
        assert exhausted, "a schedule that spent its budget skips its next window"
        ticks = _ticks(clean_graph, name)
        fired = [t for t in ticks if t["outcome"] == "fired"]
        assert fired and fired[0]["job_id"], "the first window had budget and fired"
        first = fired[0]["window_start_ms"]
        assert all(t["window_start_ms"] > first for t in exhausted)
        # A skipped tick submits no job.
        assert all(t["job_id"] == "" for t in ticks if t["outcome"] == "skipped")
        # Inside the period nothing fires again.
        in_period = [
            t for t in fired if t["window_start_ms"] < first + PERIOD_S * 1_000
        ]
        assert len(in_period) == 1

        # The next budget period has budget again and fires.
        again = _wait_for(
            lambda: [
                t
                for t in _ticks(clean_graph, name)
                if t["outcome"] == "fired"
                and t["window_start_ms"] >= first + PERIOD_S * 1_000
            ],
            40,
        )
        assert again, "the first window of the next period fires"
    finally:
        clean_graph.jobs.schedule_delete(GRAPH, name)
        clean_graph.jobs.schedule_delete(GRAPH, idle)
