"""Served tick history: outcomes, reasons, work item links and budget used.

spec: MEDULLA-PLACEMENT-R751.758

Every tick a schedule decides is recorded as `fired`, `skipped` or `refused`, with its
reason and window. A fired tick links the work item it submitted. `ScheduleTicks` answers
the last N ticks of one schedule, oldest first, each with the budget used.
"""

import time
import uuid

import pytest

GRAPH = "__commons__"
EVERY_MS = 2_000


def _wait_for(predicate, timeout_s: float):
    deadline = time.time() + timeout_s
    while time.time() < deadline:
        found = predicate()
        if found:
            return found
        time.sleep(0.5)
    return predicate()


@pytest.mark.spec("MEDULLA-PLACEMENT-R751.758")
def test_tick_history_returns_outcomes_reasons_and_work_item_links(clean_graph):
    name = f"history-{uuid.uuid4().hex[:8]}"
    now_ms = int(time.time() * 1000)
    clean_graph.jobs.schedule_create(
        GRAPH,
        name,
        {"interval": {"every_ms": EVERY_MS, "start_ms": now_ms}},
        "association.refresh",
    )
    try:
        fired = _wait_for(
            lambda: [
                tick
                for tick in clean_graph.jobs.schedule_ticks(GRAPH, name)["ticks"]
                if tick["outcome"] == "fired"
            ],
            30,
        )
        assert fired, "the first due window fires"
        assert fired[0]["work_item_id"], "a fired tick links its work item"
        assert fired[0]["reason"] == ""

        clean_graph.jobs.schedule_pause(GRAPH, name)
        skipped = _wait_for(
            lambda: [
                tick
                for tick in clean_graph.jobs.schedule_ticks(GRAPH, name)["ticks"]
                if tick["outcome"] == "skipped"
            ],
            30,
        )
        assert skipped, "a window that comes due while paused is skipped"
        assert skipped[0]["reason"] == "paused"
        assert skipped[0]["work_item_id"] is None

        history = clean_graph.jobs.schedule_ticks(GRAPH, name)
        assert history["schedule"] == name
        windows = [tick["window_start_ms"] for tick in history["ticks"]]
        assert windows == sorted(windows), "ticks come in time order"
        assert all(
            tick["used_run_s"] >= 0 and tick["used_tokens"] >= 0
            for tick in history["ticks"]
        )

        last = clean_graph.jobs.schedule_ticks(GRAPH, name, limit=1)["ticks"]
        assert last == history["ticks"][-1:], "the limit keeps the newest ticks"
    finally:
        clean_graph.jobs.schedule_delete(GRAPH, name)
