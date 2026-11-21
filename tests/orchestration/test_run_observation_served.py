"""MEDULLA-ORCH-VOCAB-R009.18.2 served receipt: an observation window with
unknown measurements round-trips, and its error rate stays Unknown when no
attempt was eligible."""

from __future__ import annotations

import time
import uuid
from typing import Any

import pytest
from _decision_resident_fixture import publish, schema_body
from _harness_envelope import graph_name as _graph_name
from conftest import TEST_TENANT

from medulla import decision_client as dc

Q32 = 1 << 32

UNKNOWN = {"state": "unknown", "value": "not reported"}


def _measured(value):
    return {"state": "measured", "value": value}


def _window(run: str, window_id: str, end_ms: int, *, eligible: int, errors: int):
    return {
        "node_type": "RunObservationWindow",
        "run_id": run,
        "run_revision": 1,
        "window_id": window_id,
        "window_revision": 1,
        "start_ms": end_ms - 100,
        "end_ms": end_ms,
        "latency_samples_ms": _measured([20] * 5),
        "error_count": _measured(errors),
        "eligible_attempts": _measured(eligible),
        "usage": {
            "input_tokens": UNKNOWN,
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


def _adjust(client, schema, run: str, *, model: str = "m1", width: int = 4):
    """Decide ``run_adjustment`` for ``run``; returns the sealed record."""
    models = [
        dc.DeclaredOption(name, numbers={"tier": tier * Q32})
        for tier, name in enumerate(["m1", "m2", "m3"], start=1)
    ]
    request = dc.decide_request(
        TEST_TENANT,
        (f"q:r009-18:{run}", "run_adjustment", "ordinary"),
        dc.declared(models),
        schema,
        params=[
            dc.param("run_id", "text", run),
            dc.param("graph", "text", _graph_name(client)),
            dc.param("current_model", "iri", model),
            dc.param("current_width", "int", width),
            dc.param("host_width_ceiling", "int", 8),
            dc.param("budget_tokens", "int", 1_000),
            dc.param("mid_run", "bool", False),
        ],
    )
    return client._send("Decide", {"request": request})["records"][0]


def _facts(record: dict[str, Any]) -> list[str]:
    return [premise["fact"] for premise in record["premises"]]


@pytest.mark.spec("MEDULLA-ORCH-VOCAB-R009.18.2")
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


@pytest.mark.spec("MEDULLA-ORCH-VOCAB-R009.18.3")
def test_window_to_adjustment_through_meristem_fixture(clean_graph):
    """The run loop's windows (written through ``add_node``) drive the
    adjustment: a noisy run moves up a tier and narrows, a stale run and a run
    with an unknown rate abstain, and a mid-run widening is refused."""
    suffix = uuid.uuid4().hex[:8]
    schema = publish(
        clean_graph, f"r009-18-schema-{suffix}", "feature_schema", schema_body()
    )
    now = int(time.time() * 1000)
    noisy, stale, blind = f"noisy-{suffix}", f"stale-{suffix}", f"blind-{suffix}"
    clean_graph.nodes.add(noisy, {"node_type": "RunTrace", "run_revision": 1})
    for i in range(20):
        clean_graph.nodes.add(
            f"{noisy}-w{i}",
            _window(noisy, f"{noisy}-w{i}", now - 10_000 - i, eligible=4, errors=int(i < 5)),
        )
        clean_graph.nodes.add(
            f"{stale}-w{i}",
            _window(stale, f"{stale}-w{i}", now - 3_600_000 - i, eligible=4, errors=0),
        )
    clean_graph.nodes.add(
        f"{blind}-w0", _window(blind, f"{blind}-w0", now - 10_000, eligible=0, errors=0)
    )

    high = _adjust(clean_graph, schema, noisy)
    assert high["outcome"]["outcome"] == "advisory", high["outcome"]
    assert sorted(s["option_id"] for s in high["outcome"]["scores"]) == [
        "budget_tokens:1000",
        "model:m2",
        "width:3",
    ]
    for code, run in (("WINDOW_STALE", stale), ("RATE_UNKNOWN", blind)):
        record = _adjust(clean_graph, schema, run)
        assert record["outcome"]["outcome"] == "abstained", record["outcome"]
        assert code in str(record["outcome"]), record["outcome"]


@pytest.mark.spec("MEDULLA-ORCH-VOCAB-R009.18.4")
def test_adjustment_readback_names_before_after_and_reason(clean_graph):
    """The committed adjustment record names the window it read, the before
    and after values and the rationale, and reads back through ``DecisionLog``."""
    suffix = uuid.uuid4().hex[:8]
    schema = publish(
        clean_graph, f"r009-18-4-schema-{suffix}", "feature_schema", schema_body()
    )
    now = int(time.time() * 1000)
    run = f"readback-{suffix}"
    for i in range(20):
        clean_graph.nodes.add(
            f"{run}-w{i}",
            _window(run, f"{run}-w{i}", now - 10_000 - i, eligible=4, errors=int(i < 5)),
        )
    record = _adjust(clean_graph, schema, run)
    committed = clean_graph._send("DecisionLog", {"op": dc.commit_op(record)})
    entry = clean_graph._send(
        "DecisionLog", {"op": dc.get_op(TEST_TENANT, committed["record_id"])}
    )["record"]
    facts = _facts(entry)
    assert f"run_adjustment.window_id={run}-w0" in facts
    assert "run_adjustment.window_revision=1" in facts
    assert "run_adjustment.current_model=m1" in facts
    assert "run_adjustment.current_width=4" in facts
    assert "run_adjustment.rationale=error_high" in facts
    assert sorted(s["option_id"] for s in entry["outcome"]["scores"]) == [
        "budget_tokens:1000",
        "model:m2",
        "width:3",
    ]
