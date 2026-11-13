"""NumericClient round trips against the Rust-emitted vectors.

The vectors in ``tests/fixtures/numeric_evaluate_vectors.json`` are written by
``crates/medulla-types/src/numeric_kernel.rs::tests::emit_round_trip_vectors`` from the
Rust ``NumericEvaluateOp`` serde type. Each vector is sent twice to the session-scoped
served engine: raw, as the Rust bytes, and through the ``client.numeric`` method of the
same name with the vector's field values. Both must return the same result.
"""

from __future__ import annotations

import json
from pathlib import Path

import pytest

VECTORS = json.loads(
    (Path(__file__).parent / "fixtures" / "numeric_evaluate_vectors.json").read_text()
)["vectors"]

RETIRED = (
    "detect_regimes",
    "information_coefficient",
    "purged_cpcv",
    "deflated_sharpe",
    "probability_backtest_overfit",
    "diebold_mariano",
    "kalman_filter_1d",
    "kalman_beta",
    "kalman_volatility",
    "adf_test",
    "ou_calibrate",
    "ou_optimal_thresholds",
    "information_ratio",
    "effective_independent_n",
    "brier_score",
)


def test_vectors_cover_all_seventeen_ops() -> None:
    assert len(VECTORS) == 17
    assert len({vector["op"] for vector in VECTORS}) == 17


# spec: MEDULLA-FINANCE-PRIMITIVES-R002.3
@pytest.mark.parametrize("vector", VECTORS, ids=lambda vector: vector["op"])
def test_round_trip_vectors_and_retired_attributes(clean_graph, vector) -> None:
    fields = {name: value for name, value in vector.items() if name != "op"}
    through_client = getattr(clean_graph.numeric, vector["op"])(**fields)
    raw = clean_graph.numeric._evaluate(vector["op"], fields)
    assert through_client == raw
    for name in RETIRED:
        with pytest.raises(AttributeError):
            getattr(clean_graph.finance, name)


# spec: MEDULLA-FINANCE-PRIMITIVES-R002.3
def test_bad_field_raises_the_engine_refusal_with_its_code(clean_graph) -> None:
    with pytest.raises(Exception, match="KERNEL_PARAM_INVALID"):
        clean_graph.numeric.information_ratio(ic="not a number", n_independent=2.0)
