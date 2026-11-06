"""Bitemporal client methods against a served engine (MEDULLA-DURABLE-KERNEL-R081).

Full path: Python client -> UDS -> Rust dispatch -> result, over the
session-scoped server and ``clean_graph`` sync client from conftest.py.
"""

from __future__ import annotations

import time

import pytest

from medulla.client import _as_of_uql

SEED = "MATCH (:Doc) WHERE year = 2020"
WALK = "TRAVERSE CITES"


def _now_ms() -> int:
    """Wall-clock milliseconds, the unit of the engine's transaction clock."""
    return time.time_ns() // 1_000_000


def _tick() -> int:
    """A millisecond strictly between the engine stamps before and after it."""
    time.sleep(0.01)
    instant = _now_ms()
    time.sleep(0.01)
    return instant


def _reached(client, ts, axis):
    result = client.edges.as_of(SEED, ts, axis=axis, then=WALK)
    return sorted(row["id"] for row in result["rows"])


@pytest.mark.spec("MEDULLA-DURABLE-KERNEL-R081")
def test_supersede_invalidate_and_as_of_end_to_end(clean_graph):
    # Transaction time is the engine clock (MEDULLA-DURABLE-KERNEL-R078): a
    # caller's tx_from / tx_now is overwritten, so the tx-axis pins bracket the
    # engine's own stamps with wall-clock instants taken between the writes.
    for node, year in (("d1", 2020), ("d2", 2021), ("d3", 2022)):
        clean_graph.nodes.add(node, {"type": "Doc", "year": year})
    before_add = _tick()
    clean_graph.edges.add("d1", "d2", {"relationship": "CITES", "valid_from": 100})
    between = _tick()
    clean_graph.edges.supersede_edge(
        "d1", "d3", "d1", "d2", "CITES", 200, _now_ms(), {"relationship": "CITES"}
    )
    after = _tick()
    cases = [
        (150, "valid", ["d2"]),
        (250, "valid", ["d3"]),
        (50, "valid", []),
        (before_add, "tx", []),
        (between, "tx", ["d2"]),
        (after, "tx", ["d3"]),
    ]
    for ts, axis, expected in cases:
        assert _reached(clean_graph, ts, axis) == expected, (ts, axis)
    closed = clean_graph.edges.invalidate_edge("d1", "d3", "CITES", 400, 450)
    assert closed == 1
    assert _reached(clean_graph, 300, "valid") == ["d3"]
    assert _reached(clean_graph, 500, "valid") == []


@pytest.mark.no_engine
def test_as_of_builder_names_the_axis_and_rejects_bad_input():
    assert _as_of_uql(SEED, 150, "valid", WALK) == f"{SEED} |> AS OF @150 |> {WALK}"
    assert _as_of_uql(SEED, 7, "tx", "") == f"{SEED} |> AS OF TX @7"
    for ts, axis in ((-1, "valid"), (1, "belief")):
        with pytest.raises(ValueError):
            _as_of_uql(SEED, ts, axis, "")
