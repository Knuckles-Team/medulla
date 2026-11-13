//! MEDULLA-FINANCE-PRIMITIVES-R002.11.0 to R002.11.3: the served pinned regime model
//! record, `Method::RegimeForecast` (p0 times T to the h, per-horizon categorical
//! Brier gate) and `Method::RegimeSizingInput` through the real signed `dispatch`
//! surface. Expected values come from an independent oracle in this file (matrix
//! powers by repeated squaring, Brier by its definition), never from the code under
//! test.
#![cfg(all(
    feature = "server",
    feature = "security",
    feature = "finance",
    feature = "shacl"
))]

mod common;
#[path = "common/test_support.rs"]
mod test_support;

use std::collections::BTreeMap;

use medulla_protocol_types::protocol::{GraphType, Method, Response, ResultPayload};
use medulla_types::regime_forecast::{
    HeldoutSample, HorizonThresholds, RegimeCalibrationInput, RegimeCalibrationPolicy,
    RegimeForecastRequest, RegimeSizingPolicy,
};
use medulla_types::wire::{FinanceRecord, FinanceRecordKind, FINANCE_V1_SCHEMA_VERSION};
use serde_json::{json, Value};

const SECRET: &str = "medulla-fin-regime-forecast-secret";
const TENANT: &str = "integration-test-tenant";
const GRAPH: &str = "fin-regime";
const AS_OF: &str = "2026-03-02T00:00:00Z";

const IDS: [&str; 3] = ["bull", "flat", "bear"];
const P0: [f64; 3] = [0.5, 0.3, 0.2];
const T: [[f64; 3]; 3] = [[0.8, 0.15, 0.05], [0.2, 0.6, 0.2], [0.1, 0.3, 0.6]];

struct Served {
    state: test_support::SharedState,
    next_id: u64,
}

fn turtle(text: &str) -> String {
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}

impl Served {
    async fn new() -> Self {
        common::configure_authority();
        let state = test_support::durable_state(SECRET, common::current_isolation());
        let mut served = Self { state, next_id: 1 };
        let created = served
            .call(Method::CreateGraph {
                graph_name: GRAPH.to_string(),
                graph_type: GraphType::Global,
            })
            .await;
        assert!(created.error.is_none(), "{:?}", created.error_detail);
        served
    }

    async fn call(&mut self, method: Method) -> Response {
        self.next_id += 1;
        test_support::dispatch(
            &self.state,
            test_support::request(SECRET, self.next_id, GRAPH, method),
        )
        .await
    }

    fn model_fields(
        revision: &str,
        source_revision: &str,
        as_of: &str,
        ids: &[&str],
        p0: &[f64],
        matrix: &[Vec<f64>],
    ) -> BTreeMap<String, String> {
        [
            ("regimeModelId", turtle("model")),
            ("regimeModelRevision", turtle(revision)),
            ("regimeSourceSeriesId", turtle("spx-daily")),
            ("regimeSourceRevision", turtle(source_revision)),
            ("regimeAsOf", turtle(as_of)),
            ("regimeStateIds", turtle(&json!(ids).to_string())),
            ("regimePosterior", turtle(&json!(p0).to_string())),
            ("regimeTransitionMatrix", turtle(&json!(matrix).to_string())),
        ]
        .into_iter()
        .map(|(name, value)| (name.to_string(), value))
        .collect()
    }

    async fn put_fields(&mut self, id: &str, fields: BTreeMap<String, String>) -> Response {
        self.call(Method::FinanceRecordPut {
            kind: FinanceRecordKind::RegimeModel,
            schema_version: FINANCE_V1_SCHEMA_VERSION.to_string(),
            record: FinanceRecord {
                record_id: id.to_string(),
                tenant: TENANT.to_string(),
                fields,
            },
            expected_head: None,
        })
        .await
    }

    async fn put_model(
        &mut self,
        id: &str,
        revision: &str,
        ids: &[&str],
        p0: &[f64],
        matrix: &[Vec<f64>],
    ) {
        let fields = Self::model_fields(revision, "s7", AS_OF, ids, p0, matrix);
        let response = self.put_fields(id, fields).await;
        assert!(
            response.error.is_none(),
            "put {id}: {:?}",
            response.error_detail
        );
    }

    async fn put_standard(&mut self, id: &str, revision: &str) {
        let matrix: Vec<Vec<f64>> = T.iter().map(|r| r.to_vec()).collect();
        self.put_model(id, revision, &IDS, &P0, &matrix).await;
    }

    async fn forecast(&mut self, request: RegimeForecastRequest) -> Response {
        self.call(Method::RegimeForecast { request }).await
    }

    async fn sizing(
        &mut self,
        request: RegimeForecastRequest,
        policy: RegimeSizingPolicy,
        forecast_digest: &str,
    ) -> Response {
        self.call(Method::RegimeSizingInput {
            request,
            policy,
            forecast_digest: forecast_digest.to_string(),
        })
        .await
    }

    async fn orders(&mut self) -> String {
        let response = self
            .call(Method::GetNodesByLabel {
                label: "Order".into(),
                after: None,
                limit: 100,
            })
            .await;
        assert!(response.error.is_none(), "{:?}", response.error_detail);
        format!("{:?}", response.result)
    }

    async fn node_count(&mut self) -> String {
        format!("{:?}", self.call(Method::NodeCount).await.result)
    }
}

fn request(model_id: &str, revision: &str, horizons: &[u32]) -> RegimeForecastRequest {
    RegimeForecastRequest {
        model_id: model_id.to_string(),
        model_revision: revision.to_string(),
        source_series_id: "spx-daily".to_string(),
        source_revision: "s7".to_string(),
        as_of: AS_OF.to_string(),
        horizons: horizons.to_vec(),
        horizon_unit: "day".to_string(),
        calibration: None,
        supplied_p0: None,
        supplied_transition_matrix: None,
    }
}

fn body(response: &Response) -> Value {
    match response.result.as_ref() {
        Some(ResultPayload::Json(value)) => value.clone(),
        Some(ResultPayload::Raw(bytes)) => rmp_serde::from_slice(bytes).expect("msgpack result"),
        other => panic!(
            "expected a body, got {other:?} / {:?}",
            response.error_detail
        ),
    }
}

fn floats(value: &Value) -> Vec<f64> {
    value
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_f64().unwrap())
        .collect()
}

/// Independent oracle: `T^h` by repeated squaring, then `p0 * T^h`.
fn oracle(h: u32) -> Vec<f64> {
    oracle_from(&P0, h)
}

fn oracle_from(p0: &[f64; 3], h: u32) -> Vec<f64> {
    fn mul(a: &[[f64; 3]; 3], b: &[[f64; 3]; 3]) -> [[f64; 3]; 3] {
        let mut out = [[0.0; 3]; 3];
        for i in 0..3 {
            for j in 0..3 {
                out[i][j] = (0..3).map(|k| a[i][k] * b[k][j]).sum();
            }
        }
        out
    }
    let mut result = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    let (mut base, mut exp) = (T, h);
    while exp > 0 {
        if exp & 1 == 1 {
            result = mul(&result, &base);
        }
        base = mul(&base, &base);
        exp >>= 1;
    }
    (0..3)
        .map(|j| (0..3).map(|i| p0[i] * result[i][j]).sum())
        .collect()
}

fn assert_refused(response: &Response, code: &str) {
    assert_eq!(
        response.error.as_deref(),
        Some(code),
        "{:?}",
        response.error_detail
    );
    assert!(response.result.is_none());
}

#[tokio::test(flavor = "multi_thread")]
// spec: MEDULLA-FINANCE-PRIMITIVES-R002.11.0
async fn pinned_model_node_round_trips_and_is_shape_checked() {
    let mut served = Served::new().await;
    served.put_standard("model", "r1").await;
    let got = served
        .call(Method::FinanceRecordGet {
            kind: FinanceRecordKind::RegimeModel,
            record_id: "model".to_string(),
        })
        .await;
    assert!(got.error.is_none(), "{:?}", got.error_detail);
    let text = body(&got).to_string();
    for needle in ["r1", "s7", "spx-daily", "bull", "regimeTransitionMatrix"] {
        assert!(text.contains(needle), "{needle} missing from {text}");
    }
    // A record without its transition matrix does not conform to RegimeModelShape.
    let mut fields = Served::model_fields("r1", "s7", AS_OF, &IDS, &P0, &[vec![1.0]]);
    fields.remove("regimeTransitionMatrix");
    let refused = served.put_fields("incomplete", fields).await;
    assert_refused(&refused, "RECORD_SHAPE_INVALID");
    // A forecast on a model that was never pinned is refused by name.
    let absent = served.forecast(request("ghost", "r1", &[1])).await;
    assert_refused(&absent, "REGIME_MODEL_NOT_FOUND");
}

#[tokio::test(flavor = "multi_thread")]
// spec: MEDULLA-FINANCE-PRIMITIVES-R002.11.1
async fn forecast_matches_independent_powers() {
    let mut served = Served::new().await;
    served.put_standard("model", "r1").await;
    let horizons = [1, 2, 5, 20, 250];
    let before = served.node_count().await;
    let response = served.forecast(request("model", "r1", &horizons)).await;
    assert!(response.error.is_none(), "{:?}", response.error_detail);
    let forecast = body(&response);
    assert_eq!(forecast["ordered_state_ids"], json!(IDS));
    assert_eq!(forecast["posterior_p0"], json!(P0));
    assert_eq!(forecast["revisions"]["model_revision"], "r1");
    assert_eq!(forecast["revisions"]["source_revision"], "s7");
    assert_eq!(forecast["as_of"], AS_OF);
    let vectors = forecast["per_horizon_probability_vectors"]
        .as_array()
        .unwrap();
    let tops = floats(&forecast["per_horizon_top_state_probability"]);
    for (index, h) in horizons.iter().enumerate() {
        let got = floats(&vectors[index]);
        let want = oracle(*h);
        for (g, w) in got.iter().zip(&want) {
            assert!((g - w).abs() < 1e-12, "h={h}: {got:?} vs {want:?}");
        }
        assert!((got.iter().sum::<f64>() - 1.0).abs() < 1e-9, "h={h} sum");
        assert!((tops[index] - want.iter().copied().fold(0.0, f64::max)).abs() < 1e-12);
    }
    let digest = forecast["forecast_digest"].as_str().unwrap().to_string();
    assert_eq!(digest.len(), 64);
    assert!(forecast["calibration"].as_array().unwrap().is_empty());
    // Reading writes nothing, and the same request answers the same digest.
    assert_eq!(served.node_count().await, before);
    let again = body(&served.forecast(request("model", "r1", &horizons)).await);
    assert_eq!(again["forecast_digest"], digest);

    // A caller cannot supply p0 or T.
    let mut supplied = request("model", "r1", &[1]);
    supplied.supplied_p0 = Some(vec![1.0, 0.0, 0.0]);
    assert_refused(&served.forecast(supplied).await, "FORECAST_INPUT_SUPPLIED");
    let mut supplied = request("model", "r1", &[1]);
    supplied.supplied_transition_matrix = Some(vec![vec![1.0]]);
    assert_refused(&served.forecast(supplied).await, "FORECAST_INPUT_SUPPLIED");

    // Horizons must be non-empty, positive and strictly increasing.
    for bad in [vec![], vec![0], vec![3, 3], vec![5, 2]] {
        let refused = served.forecast(request("model", "r1", &bad)).await;
        assert_refused(&refused, "FORECAST_HORIZON_INVALID");
    }

    // A pin that no longer matches the stored node is stale.
    let mut wrong_source = request("model", "r1", &[1]);
    wrong_source.source_revision = "s8".into();
    assert_refused(&served.forecast(wrong_source).await, "FORECAST_MODEL_STALE");
    let mut wrong_time = request("model", "r1", &[1]);
    wrong_time.as_of = "2026-03-03T00:00:00Z".into();
    assert_refused(&served.forecast(wrong_time).await, "FORECAST_MODEL_STALE");
    assert_refused(
        &served.forecast(request("model", "r0", &[1])).await,
        "FORECAST_MODEL_STALE",
    );

    // An invalid stored matrix is refused, one case per rule.
    let good: Vec<Vec<f64>> = T.iter().map(|r| r.to_vec()).collect();
    let mut negative = good.clone();
    negative[0] = vec![1.1, -0.1, 0.0];
    let mut loose = good.clone();
    loose[1][0] = 0.3;
    let cases: Vec<(&str, Vec<&str>, Vec<Vec<f64>>)> = vec![
        ("negative", IDS.to_vec(), negative),
        ("loose", IDS.to_vec(), loose),
        ("duplicate", vec!["bull", "bull", "bear"], good.clone()),
    ];
    for (name, ids, matrix) in cases {
        served.put_model(name, "r1", &ids, &P0, &matrix).await;
        let refused = served.forecast(request(name, "r1", &[1])).await;
        assert_refused(&refused, "FORECAST_MATRIX_INVALID");
    }
    // Replacing the pin with a new revision makes the old revision stale.
    let mut bumped = Served::model_fields("r2", "s7", AS_OF, &IDS, &P0, &good);
    bumped.insert("regimeModelId".into(), turtle("model"));
    let head = served
        .call(Method::FinanceRecordPut {
            kind: FinanceRecordKind::RegimeModel,
            schema_version: FINANCE_V1_SCHEMA_VERSION.to_string(),
            record: FinanceRecord {
                record_id: "model".into(),
                tenant: TENANT.into(),
                fields: bumped,
            },
            expected_head: Some(1),
        })
        .await;
    assert!(head.error.is_none(), "{:?}", head.error_detail);
    assert_refused(
        &served.forecast(request("model", "r1", &[1])).await,
        "FORECAST_MODEL_STALE",
    );
    assert!(served
        .forecast(request("model", "r2", &[1]))
        .await
        .error
        .is_none());
}

// ── the calibration gate ────────────────────────────────────────────────────────

fn day(n: u32) -> String {
    format!("2026-02-{n:02}T00:00:00Z")
}

fn sample(id: &str, origin: u32, horizon: u32, vector: [f64; 3], realized: &str) -> HeldoutSample {
    HeldoutSample {
        sample_id: id.to_string(),
        model_revision: "r1".into(),
        source_series_id: "spx-daily".into(),
        origin_as_of: day(origin),
        horizon,
        ordered_state_ids: IDS.map(String::from).to_vec(),
        predicted_probability_vector: vector.to_vec(),
        realized_state_id: realized.to_string(),
        target_observation_at: day(origin + horizon),
        scoring_revision: "score-1".into(),
    }
}

/// Three origins by two horizons.
fn samples() -> Vec<HeldoutSample> {
    vec![
        sample("a1", 10, 1, [0.7, 0.2, 0.1], "bull"),
        sample("b1", 11, 1, [0.2, 0.6, 0.2], "bull"),
        sample("c1", 12, 1, [0.1, 0.3, 0.6], "bear"),
        sample("a2", 10, 2, [0.6, 0.3, 0.1], "bull"),
        sample("b2", 11, 2, [0.4, 0.4, 0.2], "flat"),
        sample("c2", 12, 2, [0.3, 0.3, 0.4], "flat"),
    ]
}

fn thresholds(horizon: u32, max_brier: f64) -> HorizonThresholds {
    HorizonThresholds {
        horizon,
        max_brier,
        min_top_state_probability: 0.3,
        minimum_heldout_examples: 3,
    }
}

fn gated(samples: Vec<HeldoutSample>, max_brier: f64) -> RegimeForecastRequest {
    let mut req = request("model", "r1", &[1, 2]);
    req.calibration = Some(RegimeCalibrationInput {
        policy: RegimeCalibrationPolicy {
            policy_id: "regime-calibration".into(),
            revision: "p1".into(),
            horizons: vec![thresholds(1, max_brier), thresholds(2, max_brier)],
        },
        samples,
    });
    req
}

/// Brier by definition: mean over rows of the sum of squared gaps.
fn brier(rows: &[&HeldoutSample]) -> f64 {
    rows.iter()
        .map(|s| {
            IDS.iter()
                .zip(&s.predicted_probability_vector)
                .map(|(id, p)| {
                    let hit = if *id == s.realized_state_id { 1.0 } else { 0.0 };
                    (p - hit) * (p - hit)
                })
                .sum::<f64>()
        })
        .sum::<f64>()
        / rows.len() as f64
}

#[tokio::test(flavor = "multi_thread")]
// spec: MEDULLA-FINANCE-PRIMITIVES-R002.11.2
async fn brier_gate_per_horizon() {
    let mut served = Served::new().await;
    served.put_standard("model", "r1").await;
    let all = samples();
    let response = served.forecast(gated(all.clone(), 0.9)).await;
    assert!(response.error.is_none(), "{:?}", response.error_detail);
    let gate = body(&response)["calibration"].clone();
    let gate = gate.as_array().unwrap();
    assert_eq!(gate.len(), 2);
    for (index, h) in [1_u32, 2].iter().enumerate() {
        let rows: Vec<&HeldoutSample> = all.iter().filter(|s| s.horizon == *h).collect();
        let want = brier(&rows);
        assert_eq!(gate[index]["horizon"], *h);
        assert!(
            (gate[index]["brier"].as_f64().unwrap() - want).abs() < 1e-12,
            "h={h}"
        );
        assert_eq!(gate[index]["sample_count"], 3);
        assert_eq!(gate[index]["model_revision"], "r1");
        assert_eq!(gate[index]["scoring_revision"], "score-1");
        assert_eq!(gate[index]["applied_thresholds"]["max_brier"], 0.9);
        for key in [
            "dataset_digest",
            "state_ids_digest",
            "calibration_policy_digest",
        ] {
            assert_eq!(gate[index][key].as_str().unwrap().len(), 64, "{key}");
        }
        assert_eq!(gate[index]["passed"], want <= 0.9);
    }
    // A threshold the held-out score cannot meet fails the gate without a refusal.
    let tight = body(&served.forecast(gated(all.clone(), 0.01)).await);
    for horizon in tight["calibration"].as_array().unwrap() {
        assert_eq!(horizon["passed"], false);
    }

    let refuse = |label: &'static str, rows: Vec<HeldoutSample>, code: &'static str| {
        (label, gated(rows, 0.9), code)
    };
    let mut duplicate_id = samples();
    duplicate_id[1].sample_id = "a1".into();
    let mut duplicate_key = samples();
    duplicate_key[1] = sample("z1", 10, 1, [0.2, 0.6, 0.2], "bull");
    let mut leak_order = samples();
    leak_order[0].target_observation_at = day(10);
    let mut leak_future = samples();
    leak_future[0] = sample("a1", 28, 1, [0.7, 0.2, 0.1], "bull");
    leak_future[0].target_observation_at = "2026-03-09T00:00:00Z".into();
    let mut missing = samples();
    missing.remove(5);
    let mut wrong_revision = samples();
    wrong_revision[0].model_revision = "r0".into();
    let mut wrong_order = samples();
    wrong_order[0].ordered_state_ids = vec!["flat".into(), "bull".into(), "bear".into()];
    let cases = vec![
        refuse("duplicate id", duplicate_id, "CALIBRATION_ROW_DUPLICATE"),
        refuse("duplicate key", duplicate_key, "CALIBRATION_ROW_DUPLICATE"),
        refuse(
            "origin not before target",
            leak_order,
            "CALIBRATION_FUTURE_LEAK",
        ),
        refuse("target after as-of", leak_future, "CALIBRATION_FUTURE_LEAK"),
        refuse("missing row", missing, "CALIBRATION_ROW_MISSING"),
        refuse("wrong revision", wrong_revision, "CALIBRATION_ROW_MISMATCH"),
        refuse("state order", wrong_order, "CALIBRATION_ROW_MISMATCH"),
    ];
    for (label, req, code) in cases {
        let refused = served.forecast(req).await;
        assert_eq!(refused.error.as_deref(), Some(code), "{label}");
    }
    // Too few held-out rows for the policy.
    let mut low = gated(samples(), 0.9);
    for h in &mut low.calibration.as_mut().unwrap().policy.horizons {
        h.minimum_heldout_examples = 4;
    }
    assert_refused(&served.forecast(low).await, "CALIBRATION_SAMPLES_LOW");
    // Policies that are incomplete or out of range.
    let mut policies = Vec::new();
    let mut missing_horizon = gated(samples(), 0.9);
    missing_horizon
        .calibration
        .as_mut()
        .unwrap()
        .policy
        .horizons
        .pop();
    policies.push(missing_horizon);
    let mut repeated = gated(samples(), 0.9);
    repeated.calibration.as_mut().unwrap().policy.horizons[1].horizon = 1;
    policies.push(repeated);
    policies.push(gated(samples(), 2.5));
    let mut no_examples = gated(samples(), 0.9);
    no_examples.calibration.as_mut().unwrap().policy.horizons[0].minimum_heldout_examples = 0;
    policies.push(no_examples);
    let mut bad_top = gated(samples(), 0.9);
    bad_top.calibration.as_mut().unwrap().policy.horizons[0].min_top_state_probability = 1.5;
    policies.push(bad_top);
    for req in policies {
        assert_refused(&served.forecast(req).await, "CALIBRATION_POLICY_INVALID");
    }
}

// ── the sizing input ───────────────────────────────────────────────────────────

const SCALE: i64 = 1_000_000;

fn policy(weights: &[(&str, i64)], cap: i64) -> RegimeSizingPolicy {
    RegimeSizingPolicy {
        policy_id: "regime-sizing".into(),
        revision: "p1".into(),
        instrument_id: "SPX".into(),
        target_weight_by_state: weights
            .iter()
            .map(|(state, w)| (state.to_string(), *w))
            .collect(),
        unit: "fraction_of_equity".into(),
        fixed_point_scale: SCALE,
        risk_cap: cap,
    }
}

const WEIGHTS: [(&str, i64); 3] = [("bull", 800_000), ("flat", 100_000), ("bear", -400_000)];

async fn digest_of_forecast(served: &mut Served, req: &RegimeForecastRequest) -> String {
    body(&served.forecast(req.clone()).await)["forecast_digest"]
        .as_str()
        .unwrap()
        .to_string()
}

fn weights_of(response: &Response) -> Vec<i64> {
    body(response)["per_horizon"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| h["expected_target_weight"].as_i64().unwrap())
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
// spec: MEDULLA-FINANCE-PRIMITIVES-R002.11.3
async fn sizing_is_deterministic_and_policy_bound() {
    let mut served = Served::new().await;
    served.put_standard("model", "r1").await;
    let req = gated(samples(), 0.9);
    let digest = digest_of_forecast(&mut served, &req).await;
    let sized = served
        .sizing(req.clone(), policy(&WEIGHTS, 1_000_000), &digest)
        .await;
    assert!(sized.error.is_none(), "{:?}", sized.error_detail);
    let want: Vec<i64> = [1_u32, 2]
        .iter()
        .map(|h| {
            oracle(*h)
                .iter()
                .zip(WEIGHTS)
                .map(|(p, (_, w))| p * w as f64)
                .sum::<f64>()
                .round() as i64
        })
        .collect();
    assert_eq!(weights_of(&sized), want);
    assert_eq!(body(&sized)["forecast_digest"], digest);
    assert_eq!(body(&sized)["fixed_point_scale"], SCALE);
    assert_eq!(body(&sized)["unit"], "fraction_of_equity");
    // Deterministic: the same inputs give the same answer.
    let again = served
        .sizing(req.clone(), policy(&WEIGHTS, 1_000_000), &digest)
        .await;
    assert_eq!(body(&again), body(&sized));
    // Varying one state probability moves the weight exactly as the bound policy says.
    let shifted = [0.2, 0.3, 0.5];
    let matrix: Vec<Vec<f64>> = T.iter().map(|r| r.to_vec()).collect();
    served
        .put_model("model2", "r1", &IDS, &shifted, &matrix)
        .await;
    let mut other_req = req.clone();
    other_req.model_id = "model2".into();
    let other_digest = digest_of_forecast(&mut served, &other_req).await;
    let moved_p = served
        .sizing(other_req, policy(&WEIGHTS, 1_000_000), &other_digest)
        .await;
    assert!(moved_p.error.is_none(), "{:?}", moved_p.error_detail);
    let want_moved: Vec<i64> = [1_u32, 2]
        .iter()
        .map(|h| {
            oracle_from(&shifted, *h)
                .iter()
                .zip(WEIGHTS)
                .map(|(p, (_, w))| p * w as f64)
                .sum::<f64>()
                .round() as i64
        })
        .collect();
    assert_eq!(weights_of(&moved_p), want_moved);
    assert_ne!(want_moved, want);
    let orders_before = served.orders().await;
    let nodes_before = served.node_count().await;
    // The weight moves only through the bound policy: change one state's weight.
    let other = [("bull", 800_000), ("flat", 100_000), ("bear", -600_000)];
    let moved = served
        .sizing(req.clone(), policy(&other, 1_000_000), &digest)
        .await;
    assert_ne!(weights_of(&moved), want);
    assert_ne!(
        body(&moved)["sizing_policy_digest"],
        body(&sized)["sizing_policy_digest"]
    );
    // Advisory only: no order record exists and nothing else was written.
    assert_eq!(served.orders().await, orders_before);
    assert_eq!(served.node_count().await, nodes_before);

    // Every abstention reason, each with no sizing value.
    let abstain = |response: &Response, reason: &str| {
        assert_eq!(response.error.as_deref(), Some("SIZING_ABSTAIN"));
        let detail = response.error_detail.clone().unwrap_or_default();
        assert!(detail.contains(reason), "{reason} not in {detail}");
        assert!(response.result.is_none());
    };
    let missing = served
        .sizing(
            req.clone(),
            policy(&[("bull", 800_000), ("flat", 100_000)], 1_000_000),
            &digest,
        )
        .await;
    abstain(&missing, "POLICY_WEIGHT_MISSING");
    let capped = served
        .sizing(req.clone(), policy(&WEIGHTS, 1), &digest)
        .await;
    abstain(&capped, "RISK_CAP_EXCEEDED");
    let wrong_digest = served
        .sizing(req.clone(), policy(&WEIGHTS, 1_000_000), &"0".repeat(64))
        .await;
    abstain(&wrong_digest, "FORECAST_STALE");
    let mut stale = req.clone();
    stale.model_revision = "r0".into();
    let stale = served
        .sizing(stale, policy(&WEIGHTS, 1_000_000), &digest)
        .await;
    abstain(&stale, "FORECAST_STALE");
    let ungated = request("model", "r1", &[1, 2]);
    let ungated_digest = digest_of_forecast(&mut served, &ungated).await;
    let none = served
        .sizing(ungated, policy(&WEIGHTS, 1_000_000), &ungated_digest)
        .await;
    abstain(&none, "CALIBRATION_FAILED");
    let failing = gated(samples(), 0.01);
    let failing_digest = digest_of_forecast(&mut served, &failing).await;
    let failed = served
        .sizing(failing, policy(&WEIGHTS, 1_000_000), &failing_digest)
        .await;
    abstain(&failed, "CALIBRATION_FAILED");
    // A policy without a unit or a positive scale is not a policy.
    let mut broken = policy(&WEIGHTS, 1_000_000);
    broken.fixed_point_scale = 0;
    let refused = served.sizing(req, broken, &digest).await;
    assert_eq!(refused.error.as_deref(), Some("INVALID_ARGUMENT"));
}

#[tokio::test(flavor = "multi_thread")]
// spec: MEDULLA-FINANCE-PRIMITIVES-R002.11
async fn forecast_gate_sizing_end_to_end() {
    let mut served = Served::new().await;
    served.put_standard("model", "r1").await;
    let req = gated(samples(), 0.9);
    let forecast = body(&served.forecast(req.clone()).await);
    assert!(forecast["calibration"]
        .as_array()
        .unwrap()
        .iter()
        .all(|h| h["passed"] == true));
    let digest = forecast["forecast_digest"].as_str().unwrap().to_string();
    let sized = served
        .sizing(req, policy(&WEIGHTS, 1_000_000), &digest)
        .await;
    assert!(sized.error.is_none(), "{:?}", sized.error_detail);
    let vectors = forecast["per_horizon_probability_vectors"]
        .as_array()
        .unwrap();
    for (index, weight) in weights_of(&sized).iter().enumerate() {
        let expected: f64 = floats(&vectors[index])
            .iter()
            .zip(WEIGHTS)
            .map(|(p, (_, w))| p * w as f64)
            .sum();
        assert_eq!(*weight, expected.round() as i64);
    }
}
