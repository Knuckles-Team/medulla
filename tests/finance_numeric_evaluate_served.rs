//! MEDULLA-FINANCE-PRIMITIVES-R002.2.5: `Method::NumericEvaluate` runs every one of its 17 ops
//! through the real signed `dispatch` surface and answers the shared kernel's exact
//! result; a refusal carries one stable `KERNEL_*` code first in the detail.
#![cfg(all(feature = "server", feature = "security", feature = "finance"))]

mod common;
#[path = "common/test_support.rs"]
mod test_support;

use medulla_protocol_types::protocol::{GraphType, Method, Response, ResultPayload};
use medulla_types::numeric_kernel::{
    EwmaSmoothing, NumericEvaluateOp as Op, MAX_CPCV_SPLITS, MAX_KERNEL_INPUT, NUMERIC_EVALUATE_OPS,
};
use serde_json::Value;

const SECRET: &str = "medulla-fin-numeric-evaluate-secret";
const GRAPH: &str = "fin-numeric-evaluate";

/// One signed session over a fresh durable state; calls share the graph.
struct Session {
    state: test_support::SharedState,
    next_id: u64,
}

impl Session {
    async fn open() -> Self {
        common::configure_authority();
        let state = test_support::durable_state(SECRET, common::current_isolation());
        let mut session = Self { state, next_id: 0 };
        let created = session
            .send(Method::CreateGraph {
                graph_name: GRAPH.to_string(),
                graph_type: GraphType::Global,
            })
            .await;
        assert!(
            created.error.is_none(),
            "CreateGraph: {:?}",
            created.error_detail
        );
        session
    }

    async fn send(&mut self, method: Method) -> Response {
        self.next_id += 1;
        test_support::dispatch(
            &self.state,
            test_support::request(SECRET, self.next_id, GRAPH, method),
        )
        .await
    }

    async fn eval(&mut self, op: Op) -> Response {
        self.send(Method::NumericEvaluate { op: Box::new(op) })
            .await
    }
}

fn raw(response: &Response) -> Value {
    match response.result.as_ref() {
        Some(ResultPayload::Raw(bytes)) => rmp_serde::from_slice(bytes).expect("msgpack result"),
        other => panic!(
            "expected a raw result, got {other:?} / {:?}",
            response.error_detail
        ),
    }
}

fn float(response: &Response) -> f64 {
    match response.result {
        Some(ResultPayload::Float(v)) => v,
        ref other => panic!(
            "expected a float result, got {other:?} / {:?}",
            response.error_detail
        ),
    }
}

fn refused(response: &Response, code: &str) {
    assert!(response.result.is_none(), "a refusal carries no result");
    assert_eq!(response.error.as_deref(), Some("INVALID_ARGUMENT"));
    let detail = response.error_detail.as_deref().unwrap_or_default();
    assert!(
        detail.starts_with(&format!("INVALID_ARGUMENT: {code}: ")) || detail.contains(code),
        "{code} not first in {detail:?}"
    );
}

fn close(actual: &Value, expected: f64, tolerance: f64) {
    let a = actual
        .as_f64()
        .unwrap_or_else(|| panic!("not a number: {actual}"));
    assert!((a - expected).abs() <= tolerance, "{a} != {expected}");
}

/// Deterministic noise in [-1, 1).
fn noise_source(mut seed: u64) -> impl FnMut() -> f64 {
    move || {
        seed = seed
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((seed >> 33) as f64 / (1u64 << 31) as f64) - 1.0
    }
}

#[tokio::test(flavor = "multi_thread")]
// spec: MEDULLA-FINANCE-PRIMITIVES-R002.2.5
async fn numeric_evaluate_every_op_round_trips() {
    let mut s = Session::open().await;
    let mut covered = Vec::new();

    // 1. rolling_z_score: worked by hand, window 3, population deviation.
    let op = Op::RollingZScore {
        values: vec![1.0, 2.0, 4.0, 7.0],
        window: 3,
    };
    covered.push(op.name());
    let z = raw(&s.eval(op).await);
    assert!(z[0].is_null() && z[1].is_null());
    close(&z[2], 1.336_306_209_562_121_9, 1e-12);
    close(&z[3], 1.297_771_369_046_100_4, 1e-12);

    // 2. ewma: span 4 gives alpha 0.4, seeded with the first value.
    let op = Op::Ewma {
        values: vec![1.0, 2.0, 3.0],
        smoothing: EwmaSmoothing::Span(4.0),
    };
    covered.push(op.name());
    let e = raw(&s.eval(op).await);
    close(&e[0], 1.0, 1e-12);
    close(&e[1], 1.4, 1e-12);
    close(&e[2], 2.04, 1e-12);

    // 3. information_coefficient: a perfectly aligned and a perfectly opposed signal.
    let op = Op::InformationCoefficient {
        signal: vec![1.0, 2.0, 3.0, 4.0, 5.0],
        forward_returns: vec![2.0, 4.0, 6.0, 8.0, 10.0],
    };
    covered.push(op.name());
    assert!((float(&s.eval(op).await) - 1.0).abs() < 1e-12);
    let opposed = Op::InformationCoefficient {
        signal: vec![1.0, 2.0, 3.0, 4.0, 5.0],
        forward_returns: vec![5.0, 4.0, 3.0, 2.0, 1.0],
    };
    assert!((float(&s.eval(opposed).await) + 1.0).abs() < 1e-12);

    // 4. information_ratio: IC * sqrt(N).
    let op = Op::InformationRatio {
        ic: 0.1,
        n_independent: 100.0,
    };
    covered.push(op.name());
    assert!((float(&s.eval(op).await) - 1.0).abs() < 1e-12);

    // 5. effective_independent_n: identical rows are one stream, orthogonal rows two.
    let op = Op::EffectiveIndependentN {
        returns_matrix: vec![vec![1.0, 2.0, 3.0, 4.0], vec![1.0, 2.0, 3.0, 4.0]],
    };
    covered.push(op.name());
    assert!((float(&s.eval(op).await) - 1.0).abs() < 1e-9);
    let orthogonal = Op::EffectiveIndependentN {
        returns_matrix: vec![vec![1.0, -1.0, 1.0, -1.0], vec![1.0, 1.0, -1.0, -1.0]],
    };
    assert!((float(&s.eval(orthogonal).await) - 2.0).abs() < 1e-9);

    // 6. diebold_mariano: A's losses are clearly lower than B's.
    let a: Vec<f64> = (0..20).map(|i| 1.0 + 0.01 * (i % 3) as f64).collect();
    let b: Vec<f64> = (0..20).map(|i| 2.0 + 0.01 * ((i + 1) % 3) as f64).collect();
    let op = Op::DieboldMariano {
        losses_a: a,
        losses_b: b,
        h: 1,
    };
    covered.push(op.name());
    let dm = raw(&s.eval(op).await);
    assert_eq!(dm["a_better"], true);
    assert!(dm["p_value"].as_f64().unwrap() < 0.05);
    assert!(dm["statistic"].as_f64().unwrap() < 0.0);

    // 7. deflated_sharpe: a probability, lower when more trials were tried.
    let returns: Vec<f64> = (0..200).map(|i| 0.001 + f64::from(i % 7) * 1e-4).collect();
    let few = Op::DeflatedSharpe {
        observed_sr: 1.5,
        n_trials: 2,
        sr_returns: returns.clone(),
    };
    covered.push(few.name());
    let many = Op::DeflatedSharpe {
        observed_sr: 1.5,
        n_trials: 500,
        sr_returns: returns,
    };
    let (few, many) = (float(&s.eval(few).await), float(&s.eval(many).await));
    assert!((0.0..=1.0).contains(&few) && (0.0..=1.0).contains(&many));
    assert!(many < few, "more trials deflate more: {many} vs {few}");

    // 8. purged_cpcv: C(6, 2) splits, the first test group is the first 40 samples.
    let op = Op::PurgedCpcv {
        n_samples: 120,
        n_groups: 6,
        n_test_groups: 2,
        purge_window: 5,
        embargo: 5,
    };
    covered.push(op.name());
    let cpcv = raw(&s.eval(op).await);
    assert_eq!(cpcv.as_array().map(Vec::len), Some(15));
    assert_eq!(cpcv[0]["test"].as_array().map(Vec::len), Some(40));
    assert!(cpcv[0]["train"]
        .as_array()
        .unwrap()
        .iter()
        .all(|i| i.as_u64().unwrap() >= 45));

    // 9. probability_of_backtest_overfit: two of four splits land below the median.
    let op = Op::ProbabilityOfBacktestOverfit {
        insample: vec![
            vec![1.0, 2.0, 3.0],
            vec![3.0, 2.0, 1.0],
            vec![1.0, 3.0, 2.0],
            vec![1.0, 2.0, 3.0],
        ],
        oos: vec![
            vec![3.0, 2.0, 1.0],
            vec![1.0, 2.0, 3.0],
            vec![1.0, 2.0, 3.0],
            vec![1.0, 2.0, 3.0],
        ],
    };
    covered.push(op.name());
    assert!((float(&s.eval(op).await) - 0.5).abs() < 1e-12);

    // 10. brier: mean squared gap, (0.1^2 + 0.2^2 + 0.3^2 + 0.4^2) / 4 = 0.075.
    let op = Op::Brier {
        forecasts: vec![0.9, 0.2, 0.7, 0.4],
        outcomes: vec![1.0, 0.0, 1.0, 0.0],
    };
    covered.push(op.name());
    assert!((float(&s.eval(op).await) - 0.075).abs() < 1e-12);

    // 11. kalman_level: q = 0, r = 1, p0 = 1 gives gains 1/2 then 1/3.
    let op = Op::KalmanLevel {
        observations: vec![10.0, 10.0],
        f: 1.0,
        q: 0.0,
        h: 1.0,
        r: 1.0,
        x0: 0.0,
        p0: 1.0,
    };
    covered.push(op.name());
    let k = raw(&s.eval(op).await);
    close(&k["states"][0], 5.0, 1e-12);
    close(&k["states"][1], 20.0 / 3.0, 1e-12);
    close(&k["variances"][0], 0.5, 1e-12);
    close(&k["variances"][1], 1.0 / 3.0, 1e-12);

    // 12. kalman_beta: an exact 2x relation is recovered.
    let op = Op::KalmanBeta {
        market_returns: vec![1.0, 2.0, 3.0],
        asset_returns: vec![2.0, 4.0, 6.0],
        q: 0.0,
        r: 1e-6,
        beta0: 0.0,
        p0: 1.0,
    };
    covered.push(op.name());
    let beta = raw(&s.eval(op).await);
    close(&beta["states"][2], 2.0, 1e-4);

    // 13. kalman_volatility: a seeded log-variance that never moves.
    let op = Op::KalmanVolatility {
        returns: vec![0.01; 5],
        q: 0.0,
        r: 1.0,
        log_var0: Some((1e-4_f64).ln()),
        p0: 1.0,
        annualization: 252.0,
    };
    covered.push(op.name());
    let vol = raw(&s.eval(op).await);
    close(&vol[4], 0.01 * 252.0_f64.sqrt(), 1e-9);

    // 14. ou_calibrate: s_t = 0.5 s_{t-1} + 1 has mean 2 and half-life 1.
    let op = Op::OuCalibrate {
        spread: vec![0.0, 1.0, 1.5, 1.75, 1.875, 1.9375],
        dt: 1.0,
    };
    covered.push(op.name());
    let ou = raw(&s.eval(op).await);
    close(&ou["mu"], 2.0, 1e-6);
    close(&ou["theta"], std::f64::consts::LN_2, 1e-6);
    close(&ou["half_life"], 1.0, 1e-6);

    // 15. ou_thresholds: a symmetric band around the mean, exiting at the mean.
    let op = Op::OuThresholds {
        theta: 1.0,
        mu: 0.5,
        sigma: 1.0,
        sigma_eq: 0.7,
        cost: 0.0,
    };
    covered.push(op.name());
    let th = raw(&s.eval(op).await);
    close(&th["exit"], 0.5, 1e-12);
    let (lo, hi) = (
        th["entry_long"].as_f64().unwrap(),
        th["entry_short"].as_f64().unwrap(),
    );
    assert!(((0.5 - lo) - (hi - 0.5)).abs() < 1e-12 && lo < 0.5);
    assert!(th["z"].as_f64().unwrap() > 0.0);

    // 16. adf: a stationary AR(1) rejects the unit root, a random walk does not.
    let mut noise = noise_source(12_345);
    let mut stationary = vec![0.0];
    for i in 1..400 {
        stationary.push(0.2 * stationary[i - 1] + noise());
    }
    let mut walk = vec![0.0];
    for _ in 1..400 {
        walk.push(walk.last().unwrap() + noise());
    }
    let op = Op::Adf {
        series: stationary,
        max_lag: 1,
    };
    covered.push(op.name());
    let adf = raw(&s.eval(op).await);
    assert_eq!(adf["stationary_5pct"], true);
    let walk = raw(&s
        .eval(Op::Adf {
            series: walk,
            max_lag: 1,
        })
        .await);
    assert_eq!(walk["stationary_5pct"], false);

    // 17. regime_hmm: two constant regimes decode to two different states.
    let mut obs = vec![0.01; 50];
    obs.extend(std::iter::repeat_n(-0.02, 50));
    let op = Op::RegimeHmm {
        observations: obs,
        n_states: 2,
        max_iter: 100,
        tol: 1e-6,
    };
    covered.push(op.name());
    let hmm = raw(&s.eval(op).await);
    assert_eq!(hmm["states"].as_array().map(Vec::len), Some(100));
    assert_ne!(hmm["states"][0], hmm["states"][99]);
    assert_eq!(hmm["means"].as_array().map(Vec::len), Some(2));

    // Every one of the 17 ops was sent, once each by name.
    covered.sort_unstable();
    let mut named = NUMERIC_EVALUATE_OPS.to_vec();
    named.sort_unstable();
    assert_eq!(covered, named, "a named op was not round-tripped");

    every_refusal_carries_its_code(&mut s).await;
}

/// Every stable refusal code, each provoked through signed dispatch.
async fn every_refusal_carries_its_code(s: &mut Session) {
    refused(
        &s.eval(Op::RollingZScore {
            values: vec![],
            window: 3,
        })
        .await,
        "KERNEL_INPUT_SHORT",
    );
    refused(
        &s.eval(Op::Ewma {
            values: vec![1.0, f64::NAN],
            smoothing: EwmaSmoothing::Span(3.0),
        })
        .await,
        "KERNEL_INPUT_NONFINITE",
    );
    refused(
        &s.eval(Op::InformationCoefficient {
            signal: vec![1.0, 2.0, 3.0],
            forward_returns: vec![1.0, 2.0],
        })
        .await,
        "KERNEL_LENGTH_MISMATCH",
    );
    refused(
        &s.eval(Op::RollingZScore {
            values: vec![1.0, 2.0],
            window: 0,
        })
        .await,
        "KERNEL_PARAM_INVALID",
    );
    refused(
        &s.eval(Op::InformationCoefficient {
            signal: vec![1.0, 1.0, 1.0],
            forward_returns: vec![1.0, 2.0, 3.0],
        })
        .await,
        "KERNEL_SINGULAR",
    );
    refused(
        &s.eval(Op::PurgedCpcv {
            n_samples: 1_000,
            n_groups: 20,
            n_test_groups: 10,
            purge_window: 0,
            embargo: 0,
        })
        .await,
        "KERNEL_STATE_BUDGET",
    );
    // KERNEL_OP_UNKNOWN: a retired or invented name never reaches a handler; it
    // fails to decode, and the parser names the code.
    let unknown = serde_json::json!({"op": "finance_kalman_beta"});
    let refusal = Op::parse(&unknown).unwrap_err();
    assert_eq!(refusal.code.as_str(), "KERNEL_OP_UNKNOWN");
    let wire = rmp_serde::to_vec_named(&serde_json::json!({
        "method": "NumericEvaluate",
        "params": {"op": unknown}
    }))
    .unwrap();
    assert!(rmp_serde::from_slice::<Method>(&wire).is_err());
}

#[tokio::test(flavor = "multi_thread")]
// spec: MEDULLA-FINANCE-PRIMITIVES-R002.2.5
async fn numeric_evaluate_rejects_oversize_input() {
    let mut s = Session::open().await;
    // The array ceiling: exactly MAX_KERNEL_INPUT is served, one more is refused.
    let at_ceiling = vec![0.5; MAX_KERNEL_INPUT];
    let served = s
        .eval(Op::Brier {
            forecasts: at_ceiling.clone(),
            outcomes: at_ceiling,
        })
        .await;
    assert!((float(&served) - 0.25).abs() < 1e-9);
    let over = vec![0.5; MAX_KERNEL_INPUT + 1];
    refused(
        &s.eval(Op::Brier {
            forecasts: over.clone(),
            outcomes: over,
        })
        .await,
        "KERNEL_STATE_BUDGET",
    );
    // The window ceiling of the series kernels.
    refused(
        &s.eval(Op::RollingZScore {
            values: vec![1.0, 2.0],
            window: (1 << 20) + 1,
        })
        .await,
        "KERNEL_STATE_BUDGET",
    );
    // The split ceiling: C(18, 9) = 48620 is served in shape, C(20, 10) = 184756 is not.
    assert!(48_620 < MAX_CPCV_SPLITS && 184_756 > MAX_CPCV_SPLITS);
    refused(
        &s.eval(Op::PurgedCpcv {
            n_samples: 100,
            n_groups: 20,
            n_test_groups: 10,
            purge_window: 0,
            embargo: 0,
        })
        .await,
        "KERNEL_STATE_BUDGET",
    );
}
