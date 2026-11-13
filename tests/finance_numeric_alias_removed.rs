//! MEDULLA-FINANCE-PRIMITIVES-R002.2.2: the retired `Finance<Kernel>` methods are gone from the
//! wire. A request that names one fails wire decoding, while `NumericEvaluate` with the
//! matching op answers through the real signed `dispatch` surface.
#![cfg(all(feature = "server", feature = "security", feature = "finance"))]

mod common;
#[path = "common/test_support.rs"]
mod test_support;

use medulla_protocol_types::protocol::{GraphType, Method, Request};
use medulla_types::numeric_kernel::NumericEvaluateOp as Op;
use serde_json::Value;

const SECRET: &str = "medulla-fin-alias-removed-secret";
const GRAPH: &str = "fin-alias-removed";

fn vector_op(op: &str) -> Op {
    let vectors: Value =
        serde_json::from_str(include_str!("fixtures/numeric_evaluate_vectors.json"))
            .expect("vectors are JSON");
    let vector = vectors["vectors"]
        .as_array()
        .expect("vectors array")
        .iter()
        .find(|v| v["op"] == op)
        .unwrap_or_else(|| panic!("no vector for {op}"));
    Op::parse(vector).expect("vector decodes")
}

/// Signed request bytes for `NumericEvaluate`, with the method name and params swapped
/// for a retired method when `retired` is given. Nothing else changes.
fn wire_bytes(op: &Op, retired: Option<&str>) -> Vec<u8> {
    common::configure_authority();
    let request = test_support::request(
        SECRET,
        1,
        GRAPH,
        Method::NumericEvaluate {
            op: Box::new(op.clone()),
        },
    );
    let mut value: Value =
        rmp_serde::from_slice(&rmp_serde::to_vec_named(&request).expect("encode")).expect("value");
    if let Some(name) = retired {
        let mut params = value["params"]["op"].clone();
        params.as_object_mut().expect("op object").remove("op");
        value["method"] = Value::String(name.to_string());
        value["params"] = params;
    }
    rmp_serde::to_vec_named(&value).expect("encode value")
}

/// The retired name fails wire decoding; the live request carrying the same bytes in
/// every other field decodes and is answered by the engine.
async fn retired_is_undecodable_and_generic_serves(retired: &[&str], op_name: &str) {
    let op = vector_op(op_name);
    assert!(
        rmp_serde::from_slice::<Request>(&wire_bytes(&op, None)).is_ok(),
        "control request must decode"
    );
    for name in retired {
        assert!(
            rmp_serde::from_slice::<Request>(&wire_bytes(&op, Some(name))).is_err(),
            "{name} must fail wire decoding"
        );
    }
    common::configure_authority();
    let state = test_support::durable_state(SECRET, common::current_isolation());
    let created = test_support::dispatch(
        &state,
        test_support::request(
            SECRET,
            1,
            GRAPH,
            Method::CreateGraph {
                graph_name: GRAPH.to_string(),
                graph_type: GraphType::Global,
            },
        ),
    )
    .await;
    assert!(created.error.is_none(), "{:?}", created.error_detail);
    let answer = test_support::dispatch(
        &state,
        test_support::request(
            SECRET,
            2,
            GRAPH,
            Method::NumericEvaluate { op: Box::new(op) },
        ),
    )
    .await;
    assert!(answer.error.is_none(), "{:?}", answer.error_detail);
    assert!(answer.result.is_some());
}

#[tokio::test(flavor = "multi_thread")]
// spec: MEDULLA-FINANCE-PRIMITIVES-R002.2.2.3
async fn information_coefficient_legacy_method_is_undecodable() {
    retired_is_undecodable_and_generic_serves(
        &["FinanceInformationCoefficient"],
        "information_coefficient",
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
// spec: MEDULLA-FINANCE-PRIMITIVES-R002.2.2.4
async fn information_ratio_legacy_method_is_undecodable() {
    retired_is_undecodable_and_generic_serves(&["FinanceInformationRatio"], "information_ratio")
        .await;
}

#[tokio::test(flavor = "multi_thread")]
// spec: MEDULLA-FINANCE-PRIMITIVES-R002.2.2.5
async fn effective_sample_size_legacy_method_is_undecodable() {
    retired_is_undecodable_and_generic_serves(
        &["FinanceEffectiveIndependentN"],
        "effective_independent_n",
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
// spec: MEDULLA-FINANCE-PRIMITIVES-R002.2.2.6
async fn diebold_mariano_legacy_method_is_undecodable() {
    retired_is_undecodable_and_generic_serves(&["FinanceDieboldMariano"], "diebold_mariano").await;
}

#[tokio::test(flavor = "multi_thread")]
// spec: MEDULLA-FINANCE-PRIMITIVES-R002.2.2.7
async fn deflated_sharpe_legacy_method_is_undecodable() {
    retired_is_undecodable_and_generic_serves(&["FinanceDeflatedSharpe"], "deflated_sharpe").await;
}

#[tokio::test(flavor = "multi_thread")]
// spec: MEDULLA-FINANCE-PRIMITIVES-R002.2.2.8
async fn purged_cv_legacy_method_is_undecodable() {
    retired_is_undecodable_and_generic_serves(&["FinancePurgedCpcv"], "purged_cpcv").await;
}

#[tokio::test(flavor = "multi_thread")]
// spec: MEDULLA-FINANCE-PRIMITIVES-R002.2.2.9
async fn pbo_legacy_method_is_undecodable() {
    retired_is_undecodable_and_generic_serves(
        &["FinanceProbabilityBacktestOverfit"],
        "probability_of_backtest_overfit",
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
// spec: MEDULLA-FINANCE-PRIMITIVES-R002.2.2.10
async fn brier_legacy_method_is_undecodable() {
    retired_is_undecodable_and_generic_serves(&["FinanceBrierScore"], "brier").await;
}

#[tokio::test(flavor = "multi_thread")]
// spec: MEDULLA-FINANCE-PRIMITIVES-R002.2.2.11
async fn kalman_legacy_method_is_undecodable() {
    retired_is_undecodable_and_generic_serves(
        &[
            "FinanceKalmanFilter1d",
            "FinanceKalmanBeta",
            "FinanceKalmanVolatility",
        ],
        "kalman_level",
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
// spec: MEDULLA-FINANCE-PRIMITIVES-R002.2.2.12
async fn ou_legacy_method_is_undecodable() {
    retired_is_undecodable_and_generic_serves(
        &["FinanceOuCalibrate", "FinanceOuOptimalThresholds"],
        "ou_calibrate",
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
// spec: MEDULLA-FINANCE-PRIMITIVES-R002.2.2.13
async fn adf_legacy_method_is_undecodable() {
    retired_is_undecodable_and_generic_serves(&["FinanceAdfTest"], "adf").await;
}

#[tokio::test(flavor = "multi_thread")]
// spec: MEDULLA-FINANCE-PRIMITIVES-R002.2.2.14
async fn regime_hmm_legacy_method_is_undecodable() {
    retired_is_undecodable_and_generic_serves(&["FinanceDetectRegimes"], "regime_hmm").await;
}
