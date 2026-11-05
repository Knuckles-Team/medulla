"""MEDULLA-ORCH-VOCAB-R009.18.2 served receipt: an observation window with
unknown measurements round-trips, and its error rate stays Unknown when no
attempt was eligible."""

from __future__ import annotations

UNKNOWN = {"state": "unknown", "value": "not reported"}


def _measured(value):
    return {"state": "measured", "value": value}


def test_window_round_trips_with_unknowns(clean_graph):
    clean_graph.nodes.add("trace-w", {"node_type": "RunTrace", "run_revision": 1})
    window = {
        "node_type": "RunObservationWindow",
        "run_id": "trace-w",
        "run_revision": 1,
        "window_id": "window-w",
        "window_revision": 1,
        "start_ms": 0,
        "end_ms": 500,
        "latency_samples_ms": _measured([4, 6]),
        "error_count": _measured(0),
        "eligible_attempts": _measured(0),
        "usage": {
            "input_tokens": _measured(12),
            "output_tokens": UNKNOWN,
            "cache_read_tokens": UNKNOWN,
            "cache_write_tokens": UNKNOWN,
        },
        "cost": UNKNOWN,
        "model_ref": UNKNOWN,
        "profile_revision": UNKNOWN,
        "width": UNKNOWN,
        "budget_consumed": UNKNOWN,
        "budget_remaining": UNKNOWN,
        "capacity_receipt_ref": UNKNOWN,
    }
    clean_graph.nodes.add("window-w", window)
    stored = clean_graph.nodes.properties("window-w")
    assert stored["usage"]["output_tokens"] == UNKNOWN
    assert stored["error_rate"]["state"] == "unknown"
    assert stored["run_trace_ref"] == "trace-w"
    clean_graph.nodes.add("window-w", window)  # an equal re-send is idempotent
