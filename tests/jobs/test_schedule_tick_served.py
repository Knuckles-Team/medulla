"""Served schedule tick: one fire per window, only on the leader.

spec: MEDULLA-PLACEMENT-R751.753

The executor tick task reads the `Schedule` nodes of a graph and fires each due
window once, but only while it holds the leader lease record of the graph. A
rival engine that holds a live lease keeps this engine from submitting; once the
rival's lease lapses this engine takes it over with a higher fence and
acknowledges one `ScheduleTick` node per window.
"""

import time
import uuid

import pytest

LEASE = "leader_lease:schedules"
GRAPH = "__commons__"
EVERY_MS = 2_000


def _ticks(client) -> dict[str, dict]:
    ids = [row[0] for row in client.nodes.list() if row[0].startswith("schedule-tick:")]
    return {node_id: client.nodes.properties(node_id) for node_id in ids}


def _wait_for(predicate, timeout_s: float):
    deadline = time.time() + timeout_s
    while time.time() < deadline:
        found = predicate()
        if found:
            return found
        time.sleep(0.5)
    return predicate()


@pytest.mark.spec("MEDULLA-PLACEMENT-R751.753")
def test_tick_runs_once_per_window_on_leader(clean_graph):
    name = f"tick-{uuid.uuid4().hex[:8]}"
    now_ms = int(time.time() * 1000)
    rival = {
        "type": "LeaderLease",
        "state": "held",
        "holder": "rival-engine",
        "fence": 7,
        "revision": 1,
        "expires_ms": now_ms + 600_000,
    }
    clean_graph.lifecycle.batch_update(
        [{"op": "upsert_node", "id": LEASE, "properties": rival}]
    )
    clean_graph.jobs.schedule_create(
        GRAPH,
        name,
        {"Interval": {"every_ms": EVERY_MS, "start_ms": now_ms}},
        "association.refresh",
    )

    # The rival leads: windows come due and this engine submits nothing.
    time.sleep(2 * EVERY_MS / 1000 + 2)
    assert _ticks(clean_graph) == {}
    assert clean_graph.nodes.properties(LEASE)["holder"] == "rival-engine"

    # The rival's lease lapses: this engine takes the lease with a higher fence.
    clean_graph.lifecycle.batch_update(
        [
            {
                "op": "upsert_node",
                "id": LEASE,
                "properties": {**rival, "revision": 2, "expires_ms": 1},
            }
        ]
    )
    ticks = _wait_for(lambda: _ticks(clean_graph), 20)
    assert ticks, "the new leader fires the due windows"
    time.sleep(EVERY_MS / 1000 * 2 + 1)
    ticks = _ticks(clean_graph)

    lease = clean_graph.nodes.properties(LEASE)
    assert lease["holder"] != "rival-engine"
    assert lease["fence"] > 7
    windows = sorted(props["window_start_ms"] for props in ticks.values())
    # One tick per window: interval-aligned, none twice, none skipped.
    assert len(windows) == len(set(windows))
    assert all((w - now_ms) % EVERY_MS == 0 for w in windows)
    assert windows == [windows[0] + EVERY_MS * i for i in range(len(windows))]
    assert {props["host"] for props in ticks.values()} == {lease["holder"]}
    assert all(props["schedule"] == name for props in ticks.values())
