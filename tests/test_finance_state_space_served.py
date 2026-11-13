"""State-space series through the served client (FINANCE-PRIMITIVES R002.9, R002.10).

`TsDefineStateSpace` registers a versioned model over a source series; every `TsAppend`
advances the shared filter and persists the checkpoint. Fixtures are seeded and
computed here (`ss-vol-seed-7`, `ss-hedge-seed-11`), never downloaded.
"""

from __future__ import annotations

import math
import os
from typing import Any

import pytest
from conftest import request_context

KERNEL_REVISION = "medulla-series-kernels/3"
SECOND_NS = 1_000_000_000


def _eye(k: int, scale: float) -> list[float]:
    return [scale if i // k == i % k else 0.0 for i in range(k * k)]


def _model(profile: str, state_dim: int, revision: int) -> dict[str, Any]:
    return {
        "profile": profile,
        "model_revision": revision,
        "kernel_revision": KERNEL_REVISION,
        "state_dim": state_dim,
        "obs_dim": 1,
        "transition": _eye(state_dim, 1.0),
        "process_noise": _eye(state_dim, 0.01),
        "measurement_noise": [0.5],
        "initial_mean": [0.0] * state_dim,
        "initial_covariance": _eye(state_dim, 1.0),
        "noise_policy": "uncorrelated",
        "symmetry_tol": 1e-9,
        "numeric_policy": "ieee754-f64",
    }


def _lcg(seed: int):
    state = seed
    while True:
        state = (state * 6364136223846793005 + 1442695040888963407) % (1 << 64)
        yield ((state >> 33) / (1 << 31)) - 0.5


def _returns(seed: int, count: int) -> list[tuple[int, list[float]]]:
    noise = _lcg(seed)
    return [
        (i * SECOND_NS, [0.01 * (1.0 + next(noise)) + 0.002])
        for i in range(1, count + 1)
    ]


def _pairs(seed: int, count: int) -> list[tuple[int, list[float]]]:
    noise = _lcg(seed)
    rows = []
    for i in range(1, count + 1):
        regressor = (i % 11) / 10.0
        rows.append(
            (
                i * SECOND_NS,
                [0.5 + 1.5 * regressor + 0.01 * next(noise), 1.0, regressor],
            )
        )
    return rows


def _append(client, series: str, rows, width: int) -> None:
    client.timeseries.append(
        series,
        rows,
        n_fields=width,
        bucket_ns=10 * SECOND_NS,
        field_names=[f"f{i}" for i in range(width)],
    )


def _refusal(call) -> str:
    with pytest.raises(Exception) as caught:
        call()
    return str(caught.value)


# spec: MEDULLA-FINANCE-PRIMITIVES-R002.9
def test_served_filter_wiring(clean_graph) -> None:
    ts = clean_graph.timeseries
    returns = _returns(7, 40)
    _append(clean_graph, "ss_ret", returns[:20], 1)
    receipt = ts.define_state_space("ss_vol", "ss_ret", _model("volatility", 1, 1))
    assert (receipt["cursor"], receipt["appended"], receipt["caught_up"]) == (
        20,
        20,
        True,
    )
    assert receipt["model_revision"] == 1
    assert receipt["derived_from"] == "ss_ret"
    assert receipt["checkpoint_digest"].startswith("sha256:")
    _append(clean_graph, "ss_ret", returns[20:], 1)
    points = ts.range("ss_vol", 0, 100 * SECOND_NS)
    assert len(points) == 40
    for _stamp, values in points:
        assert len(values) == 3 and math.isfinite(values[0]) and values[1] == 0.0
    again = ts.define_state_space("ss_vol", "ss_ret", _model("volatility", 1, 1))
    assert (again["cursor"], again["appended"]) == (40, 0)

    _append(clean_graph, "ss_pair", _pairs(11, 60), 3)
    hedge = ts.define_state_space("ss_hedge", "ss_pair", _model("hedge", 2, 1))
    assert (hedge["cursor"], hedge["appended"]) == (60, 60)
    assert math.isfinite(ts.range("ss_hedge", 0, 100 * SECOND_NS)[-1][1][0])

    # A missing source and a model over the dimension ceiling are refused and create
    # nothing; another model over the same series leaves the checkpoint stale.
    assert "STATE_SCHEMA_UNAVAILABLE" in _refusal(
        lambda: ts.define_state_space("ss_none", "ss_missing", _model("hedge", 2, 1))
    )
    assert "KERNEL_STATE_BUDGET" in _refusal(
        lambda: ts.define_state_space("ss_big", "ss_ret", _model("factor", 17, 1))
    )
    assert ts.range("ss_big", 0, 100 * SECOND_NS) == []
    assert "STATE_CHECKPOINT_STALE" in _refusal(
        lambda: ts.define_state_space("ss_hedge", "ss_pair", _model("hedge", 2, 2))
    )


# spec: MEDULLA-FINANCE-PRIMITIVES-R002.10
def test_stream_restart_refusals_and_hot_path_receipts(clean_graph) -> None:
    from medulla.client import SyncMedullaClient

    ts = clean_graph.timeseries
    rows = _returns(7, 600)
    model = _model("volatility", 1, 1)

    # Streaming: one point at a time.
    _append(clean_graph, "hp_stream_src", rows[:1], 1)
    ts.define_state_space("hp_stream", "hp_stream_src", model)
    for row in rows[1:]:
        _append(clean_graph, "hp_stream_src", [row], 1)
    streamed = ts.define_state_space("hp_stream", "hp_stream_src", model)
    assert streamed["cursor"] == 600

    # Chunked at the checkpoint cadence (256 points), defined before the chunks.
    _append(clean_graph, "hp_chunk_src", rows[:256], 1)
    ts.define_state_space("hp_chunk", "hp_chunk_src", model)
    for start in range(256, 600, 256):
        _append(clean_graph, "hp_chunk_src", rows[start : start + 256], 1)
    chunked = ts.define_state_space("hp_chunk", "hp_chunk_src", model)
    assert chunked["cursor"] == 600
    assert chunked["checkpoint_digest"] == streamed["checkpoint_digest"]

    # Replay from a fresh connection reads the same durable checkpoint.
    fresh = SyncMedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
    )
    try:
        replay = fresh.timeseries.define_state_space(
            "hp_stream", "hp_stream_src", model
        )
        assert replay["checkpoint_digest"] == streamed["checkpoint_digest"]
        assert fresh.timeseries.range(
            "hp_stream", 0, 1000 * SECOND_NS
        ) == fresh.timeseries.range("hp_chunk", 0, 1000 * SECOND_NS)
    finally:
        fresh.close()

    # Refusals stay observable: a late point commits to the source and is refused by
    # the filter; a model that reads more fields than the source has is refused.
    _append(clean_graph, "hp_stream_src", [(SECOND_NS // 2, [0.123])], 1)
    still = ts.define_state_space("hp_stream", "hp_stream_src", model)
    assert still["cursor"] == 600
    assert "STATE_SCHEMA_UNAVAILABLE" in _refusal(
        lambda: ts.define_state_space("hp_bad", "hp_stream_src", _model("hedge", 2, 1))
    )
