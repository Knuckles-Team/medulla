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

// ---------------------------------------------------------------------------------------
// Wrapper absence and the caller census (R002.2.2.1, R002.2.2.2, R002.2.4).
// ---------------------------------------------------------------------------------------

/// One production source file under `crates/` or `src/`.
#[derive(Clone)]
struct Source {
    path: String,
    /// The file text without whole-line `//` comments.
    text: String,
}

impl Source {
    fn in_numeric(&self) -> bool {
        self.path.starts_with("crates/medulla-numeric/")
    }
}

/// Every production `.rs` file of the repository (test and bench files excluded; the
/// numeric crate included so the one definition of a kernel is visible).
fn load_tree() -> Vec<Source> {
    fn walk(root: &std::path::Path, dir: &std::path::Path, out: &mut Vec<Source>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            if path.is_dir() {
                if !matches!(name.as_str(), "target" | "tests" | "benches") {
                    walk(root, &path, out);
                }
            } else if name.ends_with(".rs")
                && !name.ends_with("tests.rs")
                && !name.ends_with("_test.rs")
            {
                let raw = std::fs::read_to_string(&path).unwrap_or_default();
                let text = raw
                    .lines()
                    .filter(|line| !line.trim_start().starts_with("//"))
                    .collect::<Vec<_>>()
                    .join("\n");
                let relative = path
                    .strip_prefix(root)
                    .expect("under the manifest dir")
                    .to_string_lossy()
                    .replace('\\', "/");
                out.push(Source {
                    path: relative,
                    text,
                });
            }
        }
    }
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut out = Vec::new();
    for top in ["crates", "src"] {
        walk(root, &root.join(top), &mut out);
    }
    out
}

fn census() -> Vec<Value> {
    serde_json::from_str::<Vec<Value>>(include_str!("fixtures/finance_kernel_callers.json"))
        .expect("census is a JSON array")
}

fn defines(source: &Source, symbol: &str) -> bool {
    source.text.contains(&format!("fn {symbol}(")) || source.text.contains(&format!("fn {symbol}<"))
}

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .expect("array")
        .iter()
        .map(|v| v.as_str().expect("string").to_string())
        .collect()
}

/// Compares the census with the tree. Every finding is a line; an empty result means the
/// census is complete: no caller is unmapped, no listed caller has vanished, no retired
/// wrapper is defined, and no kernel function is defined outside `crates/medulla-numeric`.
fn audit(census: &[Value], tree: &[Source]) -> Vec<String> {
    let valid_children: Vec<String> = (1..=14)
        .map(|n| format!("MEDULLA-FINANCE-PRIMITIVES-R002.2.1.{n}"))
        .collect();
    let mut findings = Vec::new();
    for entry in census {
        let kernel = entry["kernel"].as_str().expect("kernel name").to_string();
        let retired = entry["disposition"] == "retired";
        for symbol in strings(&entry["symbol"]) {
            for source in tree {
                if !defines(source, &symbol) {
                    continue;
                }
                if retired {
                    findings.push(format!(
                        "{kernel}: wrapper `{symbol}` is still defined in {}",
                        source.path
                    ));
                } else if !source.in_numeric() {
                    findings.push(format!(
                        "{kernel}: `{symbol}` has a second definition in {}",
                        source.path
                    ));
                }
            }
        }
        let tokens = strings(&entry["tokens"]);
        let actual: std::collections::BTreeSet<String> = tree
            .iter()
            .filter(|s| !s.in_numeric())
            .filter(|s| s.text.contains("medulla_numeric"))
            .filter(|s| tokens.iter().any(|t| s.text.contains(t.as_str())))
            .map(|s| s.path.clone())
            .collect();
        let mut listed = std::collections::BTreeSet::new();
        for caller in entry["callers"].as_array().expect("callers array") {
            let file = caller["file"].as_str().expect("file").to_string();
            let child = caller["child"].as_str().expect("child");
            if !valid_children.iter().any(|c| c == child) {
                findings.push(format!("{kernel}: {file} names an unknown child {child}"));
            }
            listed.insert(file);
        }
        for file in actual.difference(&listed) {
            findings.push(format!("{kernel}: caller {file} is not in the census"));
        }
        for file in listed.difference(&actual) {
            findings.push(format!(
                "{kernel}: census lists {file}, which no longer calls the kernel"
            ));
        }
    }
    findings
}

#[tokio::test(flavor = "multi_thread")]
// spec: MEDULLA-FINANCE-PRIMITIVES-R002.2.2.1
async fn rolling_z_score_wrapper_absent() {
    let tree = load_tree();
    assert!(
        !tree.iter().any(|s| defines(s, "rolling_zscore")),
        "the finance `rolling_zscore` wrapper must not be defined"
    );
    let entry = census()
        .into_iter()
        .find(|e| e["kernel"] == "rolling_z_score")
        .expect("census entry");
    assert!(audit(&[entry], &tree).is_empty());
    retired_is_undecodable_and_generic_serves(
        &["FinanceRollingZScore", "FinanceRollingZscore"],
        "rolling_z_score",
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
// spec: MEDULLA-FINANCE-PRIMITIVES-R002.2.2.2
async fn ewma_wrapper_absent() {
    let tree = load_tree();
    assert!(
        !tree.iter().any(|s| defines(s, "ewma_signal")),
        "the finance `ewma_signal` wrapper must not be defined"
    );
    let entry = census()
        .into_iter()
        .find(|e| e["kernel"] == "ewma")
        .expect("census entry");
    assert!(audit(&[entry], &tree).is_empty());
    retired_is_undecodable_and_generic_serves(&["FinanceEwma", "FinanceEwmaSignal"], "ewma").await;
}

#[test]
// spec: MEDULLA-FINANCE-PRIMITIVES-R002.2.4
fn complete_caller_coverage() {
    let census = census();
    assert_eq!(census.len(), 14, "one census entry per shared kernel");
    let tree = load_tree();
    let findings = audit(&census, &tree);
    assert!(findings.is_empty(), "census findings: {findings:#?}");

    // Deleting a census caller is a finding.
    let mut missing = census.clone();
    let entry = missing
        .iter_mut()
        .find(|e| e["kernel"] == "rolling_z_score")
        .expect("rolling_z_score entry");
    entry["callers"]
        .as_array_mut()
        .expect("callers")
        .pop()
        .expect("a caller to delete");
    assert!(
        audit(&missing, &tree)
            .iter()
            .any(|f| f.contains("is not in the census")),
        "a deleted census entry must fail"
    );

    // Restoring an old wrapper is a finding.
    let mut restored = tree.clone();
    restored.push(Source {
        path: "crates/medulla-finance-market/src/signals.rs".to_string(),
        text: "pub fn rolling_zscore(values: &[f64], window: usize) -> Vec<f64> { vec![] }"
            .to_string(),
    });
    assert!(
        audit(&census, &restored)
            .iter()
            .any(|f| f.contains("wrapper `rolling_zscore` is still defined")),
        "a restored wrapper must fail"
    );

    // A second implementation of a kernel outside the numeric crate is a finding.
    let mut duplicated = tree.clone();
    duplicated.push(Source {
        path: "crates/medulla-finance-quant/src/copy.rs".to_string(),
        text: "pub fn information_coefficient(a: &[f64], b: &[f64]) -> f64 { 0.0 }".to_string(),
    });
    assert!(
        audit(&census, &duplicated)
            .iter()
            .any(|f| f.contains("second definition")),
        "a second implementation must fail"
    );
}
