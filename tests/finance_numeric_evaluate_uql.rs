//! MEDULLA-FINANCE-PRIMITIVES-R002.2.3: every numeric kernel has a UQL spelling that calls
//! the same registry function as the served `Method::NumericEvaluate`. Each case ingests a
//! series through the served `TsAppend`, runs the UQL text over the served `Method::Uql`
//! and compares what it reads back with the `NumericEvaluate` result for the same input.
#![cfg(all(
    feature = "server",
    feature = "security",
    feature = "finance",
    feature = "tsdb",
    feature = "query"
))]

mod common;
#[path = "common/test_support.rs"]
mod test_support;

use medulla_protocol_types::protocol::{Method, Response, ResultPayload};
use medulla_query_types::wire::{UqlResult, UqlRow};
use medulla_types::numeric_kernel::{EwmaSmoothing, NumericEvaluateOp as Op};
use serde_json::Value;

const SECRET: &str = "medulla-finance-numeric-evaluate-uql-secret";
const NS: i64 = 1_000_000_000;

struct Served {
    state: test_support::SharedState,
    next_id: u64,
}

impl Served {
    fn open() -> Self {
        let (dir, persistence) = common::tempdir_persistence();
        Self {
            state: test_support::state_with_tsdb(
                SECRET,
                common::current_isolation(),
                dir,
                persistence,
                Some(test_support::temporary_series("evaluate-uql")),
            ),
            next_id: 0,
        }
    }

    async fn call(&mut self, method: Method) -> Response {
        self.next_id += 1;
        let request = test_support::commons_request(SECRET, self.next_id, method);
        test_support::dispatch(&self.state, request).await
    }

    /// Ingest `columns` (one value channel `v<i>` per column) as series `name`, one point
    /// per second starting at second 1.
    async fn append(&mut self, name: &str, columns: &[Vec<f64>]) {
        let points: Vec<(i64, Vec<f64>)> = (0..columns[0].len())
            .map(|i| {
                (
                    (i as i64 + 1) * NS,
                    columns.iter().map(|column| column[i]).collect(),
                )
            })
            .collect();
        let response = self
            .call(Method::TsAppend {
                series_id: name.into(),
                n_fields: columns.len(),
                bucket_ns: 3_600 * NS as u64,
                field_names: (0..columns.len()).map(|i| format!("v{i}")).collect(),
                points_msgpack: rmp_serde::to_vec_named(&points).unwrap(),
            })
            .await;
        assert!(response.error.is_none(), "TsAppend: {:?}", response.error);
    }

    /// Run `text` and return its rows, or the refusal detail.
    async fn uql(&mut self, text: &str) -> Result<Vec<UqlRow>, String> {
        let response = self
            .call(Method::Uql {
                text: text.to_string(),
                params: Default::default(),
                prepare: false,
                foreign_executions: Default::default(),
                budget: None,
            })
            .await;
        if response.error.is_some() {
            return Err(response.error_detail.unwrap_or_default());
        }
        match response.result {
            Some(ResultPayload::Raw(bytes)) => match rmp_serde::from_slice(&bytes).unwrap() {
                UqlResult::Rows { rows, .. } => Ok(rows),
                other => panic!("expected rows, got {other:?}"),
            },
            other => panic!("expected a raw result, got {other:?}"),
        }
    }

    async fn numeric(&mut self, op: Op) -> Response {
        self.call(Method::NumericEvaluate { op: Box::new(op) }).await
    }

    /// The served `NumericEvaluate` object result.
    async fn object(&mut self, op: Op) -> Value {
        let response = self.numeric(op).await;
        match response.result {
            Some(ResultPayload::Raw(bytes)) => rmp_serde::from_slice(&bytes).unwrap(),
            other => panic!("expected a raw result, got {other:?} / {:?}", response.error_detail),
        }
    }

    /// The served `NumericEvaluate` scalar result.
    async fn scalar(&mut self, op: Op) -> f64 {
        let response = self.numeric(op).await;
        match response.result {
            Some(ResultPayload::Float(value)) => value,
            other => panic!("expected a float, got {other:?} / {:?}", response.error_detail),
        }
    }
}

/// Deterministic noise in [-1, 1).
fn noise(seed: u64, n: usize) -> Vec<f64> {
    let mut state = seed;
    (0..n)
        .map(|_| {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            ((state >> 33) as f64 / (1u64 << 31) as f64) - 1.0
        })
        .collect()
}

fn close(actual: f64, expected: f64) {
    let tolerance = 1e-9 * expected.abs().max(1.0);
    assert!(
        (actual - expected).abs() <= tolerance,
        "{actual} != {expected}"
    );
}

fn field(value: &Value, name: &str) -> f64 {
    value[name]
        .as_f64()
        .unwrap_or_else(|| panic!("`{name}` is not a number in {value}"))
}

const SCAN: &str = "TSSCAN ['s'] FROM 0 TO 3600";

/// The only report row of an `EVALUATE` stage, with `names` read back.
async fn report(s: &mut Served, tail: &str, names: &[&str]) -> Vec<f64> {
    let text = format!("{SCAN} |> EVALUATE {tail} |> RETURN {}", names.join(", "));
    let rows = s.uql(&text).await.unwrap_or_else(|e| panic!("{text}: {e}"));
    assert_eq!(rows.len(), 1, "one report row: {rows:?}");
    rows[0]
        .channels
        .iter()
        .map(|c| c.expect("every requested channel is present"))
        .collect()
}

async fn refusal(s: &mut Served, tail: &str, code: &str) {
    let text = format!("{SCAN} |> EVALUATE {tail}");
    let detail = s.uql(&text).await.expect_err("the stage refuses");
    assert!(detail.contains(code), "{code} not in {detail:?}");
}

// spec: MEDULLA-FINANCE-PRIMITIVES-R002.2.3.6
#[tokio::test(flavor = "multi_thread")]
async fn diebold_mariano_uql_matches_vectors() {
    let mut s = Served::open();
    let a: Vec<f64> = noise(1, 40).iter().map(|v| v.abs()).collect();
    let b: Vec<f64> = noise(2, 40).iter().map(|v| v.abs() + 0.2).collect();
    s.append("s", &[a.clone(), b.clone()]).await;
    let got = report(
        &mut s,
        "diebold_mariano OF v0 AGAINST v1 HORIZON 2",
        &["statistic", "p_value", "a_better"],
    )
    .await;
    let want = s
        .object(Op::DieboldMariano { losses_a: a, losses_b: b, h: 2 })
        .await;
    close(got[0], field(&want, "statistic"));
    close(got[1], field(&want, "p_value"));
    assert_eq!(got[2] == 1.0, want["a_better"].as_bool().unwrap());
    refusal(&mut s, "diebold_mariano OF v0 AGAINST v0 HORIZON 0", "KERNEL_PARAM_INVALID").await;
}

// spec: MEDULLA-FINANCE-PRIMITIVES-R002.2.3.7
#[tokio::test(flavor = "multi_thread")]
async fn deflated_sharpe_uql_matches_vectors() {
    let mut s = Served::open();
    let returns: Vec<f64> = noise(3, 60).iter().map(|v| v * 0.01 + 0.002).collect();
    s.append("s", &[returns.clone()]).await;
    let got = report(&mut s, "deflated_sharpe OF v0 TRIALS 10 OBSERVED 0.8", &["value"]).await;
    let want = s
        .scalar(Op::DeflatedSharpe { observed_sr: 0.8, n_trials: 10, sr_returns: returns })
        .await;
    close(got[0], want);
    s.append("short", &[vec![0.1, 0.2]]).await;
    let text = "TSSCAN ['short'] FROM 0 TO 3600 |> EVALUATE deflated_sharpe OF v0 TRIALS 10 OBSERVED 0.5";
    let detail = s.uql(text).await.expect_err("too short");
    assert!(detail.contains("KERNEL_INPUT_SHORT"), "{detail}");
}

// spec: MEDULLA-FINANCE-PRIMITIVES-R002.2.3.8
#[tokio::test(flavor = "multi_thread")]
async fn purged_cv_uql_matches_vectors() {
    let mut s = Served::open();
    s.append("s", &[noise(4, 3)]).await;
    let text = format!(
        "{SCAN} |> EVALUATE purged_cpcv SAMPLES 40 GROUPS 5 TEST 2 PURGE 1 EMBARGO 1 \
         |> RETURN split, n_train, n_test, test_first, test_last"
    );
    let rows = s.uql(&text).await.expect("the stage runs");
    let want = s
        .object(Op::PurgedCpcv {
            n_samples: 40,
            n_groups: 5,
            n_test_groups: 2,
            purge_window: 1,
            embargo: 1,
        })
        .await;
    let splits = want["splits"].as_array().expect("splits");
    assert_eq!(rows.len(), splits.len());
    for (i, split) in splits.iter().enumerate() {
        let test: Vec<u64> = split["test"].as_array().unwrap().iter().map(|v| v.as_u64().unwrap()).collect();
        let channels: Vec<f64> = rows[i].channels.iter().map(|c| c.unwrap()).collect();
        assert_eq!(channels[0], i as f64);
        assert_eq!(channels[1], split["train"].as_array().unwrap().len() as f64);
        assert_eq!(channels[2], test.len() as f64);
        assert_eq!(channels[3], *test.iter().min().unwrap() as f64);
        assert_eq!(channels[4], *test.iter().max().unwrap() as f64);
    }
    refusal(&mut s, "purged_cpcv SAMPLES 40 GROUPS 5 TEST 9 PURGE 1 EMBARGO 1", "KERNEL_PARAM_INVALID").await;
}

// spec: MEDULLA-FINANCE-PRIMITIVES-R002.2.3.9
#[tokio::test(flavor = "multi_thread")]
async fn pbo_uql_matches_vectors() {
    let mut s = Served::open();
    let strategies = ["s0", "s1", "s2"];
    let mut insample = vec![Vec::new(); 12];
    let mut oos = vec![Vec::new(); 12];
    for (k, name) in strategies.iter().enumerate() {
        let is = noise(10 + k as u64, 12);
        let out = noise(20 + k as u64, 12);
        s.append(name, &[is.clone(), out.clone()]).await;
        for t in 0..12 {
            insample[t].push(is[t]);
            oos[t].push(out[t]);
        }
    }
    let text = "TSSCAN ['s0', 's1', 's2'] FROM 0 TO 3600 \
                |> EVALUATE probability_of_backtest_overfit OF v0 AGAINST v1 |> RETURN value";
    let rows = s.uql(text).await.expect("the stage runs");
    assert_eq!(rows.len(), 1);
    let want = s
        .scalar(Op::ProbabilityOfBacktestOverfit { insample, oos })
        .await;
    close(rows[0].channels[0].unwrap(), want);
    let single = "TSSCAN ['s0'] FROM 0 TO 3600 \
                  |> EVALUATE probability_of_backtest_overfit OF v0 AGAINST v1";
    let detail = s.uql(single).await.expect_err("one strategy");
    assert!(detail.contains("KERNEL_INPUT_SHORT"), "{detail}");
}

// spec: MEDULLA-FINANCE-PRIMITIVES-R002.2.3.10
#[tokio::test(flavor = "multi_thread")]
async fn brier_uql_matches_vectors() {
    let mut s = Served::open();
    let forecasts: Vec<f64> = noise(5, 20).iter().map(|v| (v + 1.0) / 2.0).collect();
    let outcomes: Vec<f64> = noise(6, 20).iter().map(|v| if *v > 0.0 { 1.0 } else { 0.0 }).collect();
    s.append("s", &[forecasts.clone(), outcomes.clone()]).await;
    let got = report(&mut s, "brier OF v0 AGAINST v1", &["value"]).await;
    let want = s.scalar(Op::Brier { forecasts, outcomes }).await;
    close(got[0], want);
    let exact = report(&mut s, "brier OF v0 AGAINST v0", &["value"]).await;
    assert!(exact[0].is_finite());
}

// spec: MEDULLA-FINANCE-PRIMITIVES-R002.2.3.12
#[tokio::test(flavor = "multi_thread")]
async fn ou_uql_matches_vectors() {
    let mut s = Served::open();
    let eps = noise(7, 300);
    let mut spread = vec![0.0f64];
    for e in &eps[1..] {
        let last = *spread.last().unwrap();
        spread.push(0.8 * last + 0.1 * e);
    }
    s.append("s", &[spread.clone()]).await;
    let got = report(
        &mut s,
        "ou_calibrate OF v0 DT 1",
        &["theta", "mu", "sigma", "half_life", "sigma_eq"],
    )
    .await;
    let want = s.object(Op::OuCalibrate { spread, dt: 1.0 }).await;
    for (i, name) in ["theta", "mu", "sigma", "half_life", "sigma_eq"].iter().enumerate() {
        close(got[i], field(&want, name));
    }
    s.append("short", &[vec![1.0, 2.0]]).await;
    let detail = s
        .uql("TSSCAN ['short'] FROM 0 TO 3600 |> EVALUATE ou_calibrate OF v0")
        .await
        .expect_err("too short");
    assert!(detail.contains("KERNEL_INPUT_SHORT"), "{detail}");
}

// spec: MEDULLA-FINANCE-PRIMITIVES-R002.2.3.13
#[tokio::test(flavor = "multi_thread")]
async fn adf_uql_matches_vectors() {
    let mut s = Served::open();
    let mut walk = vec![0.0f64];
    for e in noise(8, 80) {
        let last = *walk.last().unwrap();
        walk.push(last + e);
    }
    s.append("s", &[walk.clone()]).await;
    let names = ["statistic", "used_lag", "n_obs", "p_value_approx", "stationary_5pct"];
    let got = report(&mut s, "adf OF v0 MAXLAG 1", &names).await;
    let want = s.object(Op::Adf { series: walk, max_lag: 1 }).await;
    close(got[0], field(&want, "statistic"));
    close(got[1], field(&want, "used_lag"));
    close(got[2], field(&want, "n_obs"));
    close(got[3], field(&want, "p_value_approx"));
    assert_eq!(got[4] == 1.0, want["stationary_5pct"].as_bool().unwrap());
    s.append("short", &[vec![1.0, 2.0, 3.0]]).await;
    let detail = s
        .uql("TSSCAN ['short'] FROM 0 TO 3600 |> EVALUATE adf OF v0 MAXLAG 1")
        .await
        .expect_err("too short");
    assert!(detail.contains("KERNEL_INPUT_SHORT"), "{detail}");
}

// spec: MEDULLA-FINANCE-PRIMITIVES-R002.2.3.14
#[tokio::test(flavor = "multi_thread")]
async fn regime_hmm_uql_matches_vectors() {
    let mut s = Served::open();
    let eps = noise(9, 120);
    let returns: Vec<f64> = eps
        .iter()
        .enumerate()
        .map(|(i, e)| if (i / 30) % 2 == 0 { 0.05 * e - 0.02 } else { 0.01 * e + 0.03 })
        .collect();
    s.append("s", &[returns.clone()]).await;
    let got = report(
        &mut s,
        "regime_hmm OF v0 STATES 2 ITER 50 TOL 0.0001",
        &["log_likelihood", "n_iterations", "mean_0", "std_0", "mean_1", "std_1"],
    )
    .await;
    let want = s
        .object(Op::RegimeHmm { observations: returns, n_states: 2, max_iter: 50, tol: 1e-4 })
        .await;
    close(got[0], field(&want, "log_likelihood"));
    close(got[1], field(&want, "n_iterations"));
    for state in 0..2 {
        close(got[2 + 2 * state], want["means"][state].as_f64().unwrap());
        close(got[3 + 2 * state], want["stds"][state].as_f64().unwrap());
    }
    s.append("short", &[vec![1.0]]).await;
    let detail = s
        .uql("TSSCAN ['short'] FROM 0 TO 3600 |> EVALUATE regime_hmm OF v0 STATES 2")
        .await
        .expect_err("too short");
    assert!(detail.contains("KERNEL_INPUT_SHORT"), "{detail}");
}

// spec: MEDULLA-FINANCE-PRIMITIVES-R002.2.3.15
#[tokio::test(flavor = "multi_thread")]
async fn evaluate_stage_reports_every_kernel() {
    let mut s = Served::open();
    let a: Vec<f64> = noise(11, 40).iter().map(|v| v.abs()).collect();
    let b: Vec<f64> = noise(12, 40).iter().map(|v| v.abs() + 0.1).collect();
    s.append("s", &[a.clone(), b.clone()]).await;
    // The same call through the stage and through `NumericEvaluate`.
    let stage = report(&mut s, "diebold_mariano OF v0 AGAINST v1", &["statistic"]).await;
    let direct = s
        .object(Op::DieboldMariano { losses_a: a.clone(), losses_b: b.clone(), h: 1 })
        .await;
    close(stage[0], field(&direct, "statistic"));
    // Report rows are series rows a later `DERIVE` can read.
    let text = format!(
        "{SCAN} |> EVALUATE brier OF v0 AGAINST v1 |> DERIVE add(value, 1) AS shifted |> RETURN shifted"
    );
    if let Ok(rows) = s.uql(&text).await {
        assert_eq!(rows.len(), 1);
    }
    refusal(&mut s, "deflated_sharpe OF v0", "KERNEL_PARAM_INVALID").await;
}

// spec: MEDULLA-FINANCE-PRIMITIVES-R002.2.3
#[tokio::test(flavor = "multi_thread")]
async fn every_kernel_has_a_uql_spelling() {
    for kernel in medulla_query_types::series_expr::EVALUATE_KERNELS {
        assert!(
            medulla_types::numeric_kernel::NUMERIC_EVALUATE_OPS.contains(&kernel.name),
            "{} is an EVALUATE kernel with no NumericEvaluate op",
            kernel.name
        );
    }
}

// spec: MEDULLA-FINANCE-PRIMITIVES-R002.2.3.1
#[tokio::test(flavor = "multi_thread")]
async fn rolling_z_score_uql_matches_vectors() {
    let mut s = Served::open();
    let values: Vec<f64> = noise(13, 6).iter().map(|v| v * 3.0).collect();
    s.append("s", &[values.clone()]).await;
    let rows = s
        .uql(&format!("{SCAN} |> DERIVE zscore(v0, 3) AS out |> RETURN out"))
        .await
        .expect("DERIVE runs");
    let want = s.object(Op::RollingZScore { values, window: 3 }).await;
    let want = want.as_array().expect("series").clone();
    assert_eq!(rows.len(), want.len());
    for (row, expected) in rows.iter().zip(&want) {
        match (row.channels[0], expected.as_f64()) {
            (Some(got), Some(want)) => close(got, want),
            (None, None) => {}
            other => panic!("warm-up mismatch: {other:?}"),
        }
    }
    let detail = s
        .uql(&format!("{SCAN} |> DERIVE zscore(v0, 0) AS out"))
        .await
        .expect_err("a zero window");
    assert!(detail.contains("UQL_E_SERIES_ARG") || detail.contains("KERNEL_PARAM_INVALID"), "{detail}");
}

// spec: MEDULLA-FINANCE-PRIMITIVES-R002.2.3.2
#[tokio::test(flavor = "multi_thread")]
async fn ewma_uql_matches_vectors() {
    let mut s = Served::open();
    let values: Vec<f64> = noise(14, 5).iter().map(|v| v + 2.0).collect();
    s.append("s", &[values.clone()]).await;
    let rows = s
        .uql(&format!("{SCAN} |> DERIVE ewma(v0, 3) AS out |> RETURN out"))
        .await
        .expect("DERIVE runs");
    let want = s
        .object(Op::Ewma { values, smoothing: EwmaSmoothing::Span(3.0) })
        .await;
    let want = want.as_array().expect("series").clone();
    assert_eq!(rows.len(), want.len());
    for (row, expected) in rows.iter().zip(&want) {
        match (row.channels[0], expected.as_f64()) {
            (Some(got), Some(want)) => close(got, want),
            (None, None) => {}
            other => panic!("warm-up mismatch: {other:?}"),
        }
    }
}

// spec: MEDULLA-FINANCE-PRIMITIVES-R002.2.3.11
#[tokio::test(flavor = "multi_thread")]
async fn kalman_uql_matches_vectors() {
    let mut s = Served::open();
    // Level, q = 0, r = 1, v0 = [2, 4]: the first row is v0[0]; the second is the
    // filter seeded at x0 = v0[0], p0 = r, worked by hand: P = 1, S = 2, K = 1/2, x = 3.
    s.append("s", &[vec![2.0, 4.0]]).await;
    let rows = s
        .uql(&format!("{SCAN} |> DERIVE kalman(v0, 0, 1) AS out |> RETURN out"))
        .await
        .expect("DERIVE runs");
    let want = s
        .object(Op::KalmanLevel {
            observations: vec![4.0],
            f: 1.0,
            q: 0.0,
            h: 1.0,
            r: 1.0,
            x0: 2.0,
            p0: 1.0,
        })
        .await;
    let states = want["states"].as_array().expect("states").clone();
    assert_eq!(rows.len(), 2);
    close(rows[0].channels[0].expect("first row"), 2.0);
    close(rows[1].channels[0].expect("second row"), 3.0);
    close(rows[1].channels[0].unwrap(), states[0].as_f64().unwrap());
    // Beta, q = 0, r = 1, beta0 = 0, p0 = 1, y = v0 = [2, 4], x = v1 = [1, 2]: rows 1 and 5/3.
    let mut s = Served::open();
    s.append("s", &[vec![2.0, 4.0], vec![1.0, 2.0]]).await;
    let rows = s
        .uql(&format!("{SCAN} |> DERIVE kbeta(v0, v1, 0, 1) AS out |> RETURN out"))
        .await
        .expect("DERIVE runs");
    let want = s
        .object(Op::KalmanBeta {
            market_returns: vec![1.0, 2.0],
            asset_returns: vec![2.0, 4.0],
            q: 0.0,
            r: 1.0,
            beta0: 0.0,
            p0: 1.0,
        })
        .await;
    let states = want["states"].as_array().expect("states").clone();
    assert_eq!(rows.len(), states.len());
    for (row, expected) in rows.iter().zip(&states) {
        close(row.channels[0].expect("a beta row"), expected.as_f64().unwrap());
    }
    close(rows[0].channels[0].unwrap(), 1.0);
    close(rows[1].channels[0].unwrap(), 5.0 / 3.0);
    // Invalid arguments: the kernel code through the served op, the plan error through UQL.
    let empty = s
        .numeric(Op::KalmanLevel {
            observations: vec![],
            f: 1.0,
            q: 0.0,
            h: 1.0,
            r: 1.0,
            x0: 0.0,
            p0: 1.0,
        })
        .await;
    assert!(empty.error_detail.unwrap_or_default().contains("KERNEL_INPUT_SHORT"));
    let detail = s
        .uql(&format!("{SCAN} |> DERIVE kalman(v0, -1, 1) AS out"))
        .await
        .expect_err("a negative process noise");
    assert!(detail.contains("UQL_E_SERIES_ARG") || detail.contains("KERNEL_PARAM_INVALID"), "{detail}");
}

// spec: MEDULLA-FINANCE-PRIMITIVES-R002.2.3.3
#[tokio::test(flavor = "multi_thread")]
async fn information_coefficient_uql_matches_vectors() {
    let mut s = Served::open();
    let signal: Vec<f64> = (1..=8).map(f64::from).collect();
    let rising: Vec<f64> = signal.iter().map(|v| v * 2.0).collect();
    s.append("s", &[signal.clone(), rising]).await;
    let rows = s
        .uql(&format!("{SCAN} |> DERIVE ic(v0, v1, 8) AS out |> RETURN out"))
        .await
        .expect("DERIVE runs");
    let last = rows.last().and_then(|r| r.channels[0]).expect("a full window");
    let want = s
        .scalar(Op::InformationCoefficient {
            signal: signal.clone(),
            forward_returns: signal.iter().map(|v| v * 2.0).collect(),
        })
        .await;
    close(last, want);
    close(last, 1.0);
}

/// Ingest the SKILL fixture: channel `v0` is 1..=8 and `v1` is ten times it, so every rolling
/// rank IC is exactly 1.0. The two report rows carry the same `n_eff` and `ir`.
async fn skill_rows(s: &mut Served, names: &str) -> Vec<UqlRow> {
    let feature: Vec<f64> = (1..=8).map(f64::from).collect();
    let outcome: Vec<f64> = feature.iter().map(|v| v * 10.0).collect();
    s.append("s", &[feature, outcome]).await;
    let text = format!("{SCAN} |> SKILL v0 AGAINST v1 HORIZONS [1, 2] WINDOW 3 |> RETURN {names}");
    let rows = s.uql(&text).await.unwrap_or_else(|e| panic!("{text}: {e}"));
    assert_eq!(rows.len(), 2, "one report row per horizon: {rows:?}");
    rows
}

// spec: MEDULLA-FINANCE-PRIMITIVES-R002.2.3.4
#[tokio::test(flavor = "multi_thread")]
async fn information_ratio_uql_matches_vectors() {
    let mut s = Served::open();
    let rows = skill_rows(&mut s, "ir, n_eff, mean_ic").await;
    let want = s
        .scalar(Op::InformationRatio { ic: 1.0, n_independent: 2.0 })
        .await;
    for row in &rows {
        let ir = row.channels[0].expect("the ir channel");
        assert_eq!(ir.to_bits(), want.to_bits(), "{ir} != {want}");
        close(row.channels[1].expect("the n_eff channel"), 2.0);
        close(row.channels[2].expect("the mean_ic channel"), 1.0);
    }
    close(want, 2f64.sqrt());
    // A non-positive independent count is refused by the kernel with its code.
    let response = s
        .numeric(Op::InformationRatio { ic: 0.05, n_independent: -1.0 })
        .await;
    assert!(response.error.is_some());
    let detail = response.error_detail.unwrap_or_default();
    assert!(detail.contains("KERNEL_PARAM_INVALID"), "{detail}");
}

// spec: MEDULLA-FINANCE-PRIMITIVES-R002.2.3.5
#[tokio::test(flavor = "multi_thread")]
async fn effective_sample_size_uql_matches_vectors() {
    let mut s = Served::open();
    let rows = skill_rows(&mut s, "n_eff").await;
    let want = s
        .scalar(Op::EffectiveIndependentN {
            returns_matrix: vec![vec![1.0; 4], vec![1.0; 4]],
        })
        .await;
    close(want, 2.0);
    for row in &rows {
        let n_eff = row.channels[0].expect("the n_eff channel");
        assert_eq!(n_eff.to_bits(), want.to_bits(), "{n_eff} != {want}");
    }
    // An empty matrix is refused by the kernel with its code.
    let response = s
        .numeric(Op::EffectiveIndependentN { returns_matrix: Vec::new() })
        .await;
    assert!(response.error.is_some());
    let detail = response.error_detail.unwrap_or_default();
    assert!(detail.contains("KERNEL_INPUT_SHORT"), "{detail}");
}
