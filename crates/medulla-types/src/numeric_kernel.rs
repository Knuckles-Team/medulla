//! Wire types of the served numeric kernel evaluation (MEDULLA-FINANCE-PRIMITIVES-R002.2.5).
//!
//! `Method::NumericEvaluate` carries one [`NumericEvaluateOp`]. The field names of
//! each op are the field names of the `Finance<Kernel>` method it replaces, so a
//! caller maps one to the other without renaming. Evaluation is pure compute over
//! what the request carries: no store, no clock, no graph core. A refusal is a
//! [`KernelRefusal`] whose stable code is rendered first in the error detail.

use std::fmt;

use serde::{Deserialize, Serialize};

/// Most values any one input array of an op may hold.
pub const MAX_KERNEL_INPUT: usize = 1_000_000;
/// Most observations a `regime_hmm` refit may take (the fit is iterative, so it is
/// bounded well below the plain array ceiling).
pub const MAX_REFIT_OBSERVATIONS: usize = 100_000;
/// Most splits a `purged_cpcv` op may produce.
pub const MAX_CPCV_SPLITS: usize = 100_000;

/// EWMA smoothing: by span or by half-life, in observations.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum EwmaSmoothing {
    /// Span, at least one.
    Span(f64),
    /// Half-life, above zero.
    HalfLife(f64),
}

/// One numeric kernel evaluation. 17 ops cover the shared kernel forms.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum NumericEvaluateOp {
    /// Rolling z-score of `values` over `window`; `None` before the window fills.
    RollingZScore { values: Vec<f64>, window: usize },
    /// Exponentially weighted moving average of `values`.
    Ewma {
        values: Vec<f64>,
        smoothing: EwmaSmoothing,
    },
    /// Rank correlation of a signal with the returns that followed it.
    InformationCoefficient {
        signal: Vec<f64>,
        forward_returns: Vec<f64>,
    },
    /// `ic * sqrt(n_independent)`.
    InformationRatio { ic: f64, n_independent: f64 },
    /// Effective number of independent return streams.
    EffectiveIndependentN { returns_matrix: Vec<Vec<f64>> },
    /// Diebold-Mariano test of equal predictive accuracy.
    DieboldMariano {
        losses_a: Vec<f64>,
        losses_b: Vec<f64>,
        h: usize,
    },
    /// Deflated Sharpe ratio after `n_trials` trials.
    DeflatedSharpe {
        observed_sr: f64,
        n_trials: usize,
        sr_returns: Vec<f64>,
    },
    /// Purged, embargoed combinatorial cross-validation splits.
    PurgedCpcv {
        n_samples: usize,
        n_groups: usize,
        n_test_groups: usize,
        purge_window: usize,
        embargo: usize,
    },
    /// Probability of backtest overfitting from per-split performance.
    ProbabilityOfBacktestOverfit {
        insample: Vec<Vec<f64>>,
        oos: Vec<Vec<f64>>,
    },
    /// Brier score of probability forecasts against binary outcomes.
    Brier {
        forecasts: Vec<f64>,
        outcomes: Vec<f64>,
    },
    /// Scalar Kalman filter with constant matrices.
    KalmanLevel {
        observations: Vec<f64>,
        f: f64,
        q: f64,
        h: f64,
        r: f64,
        x0: f64,
        p0: f64,
    },
    /// Dynamic regression coefficient of asset returns on market returns.
    KalmanBeta {
        market_returns: Vec<f64>,
        asset_returns: Vec<f64>,
        q: f64,
        r: f64,
        beta0: f64,
        p0: f64,
    },
    /// Annualised volatility from a log-variance Kalman state.
    KalmanVolatility {
        returns: Vec<f64>,
        q: f64,
        r: f64,
        log_var0: Option<f64>,
        p0: f64,
        annualization: f64,
    },
    /// Ornstein-Uhlenbeck calibration of a spread sampled every `dt`.
    OuCalibrate { spread: Vec<f64>, dt: f64 },
    /// Entry and exit thresholds that maximise expected return per unit time.
    OuThresholds {
        theta: f64,
        mu: f64,
        sigma: f64,
        sigma_eq: f64,
        cost: f64,
    },
    /// Augmented Dickey-Fuller test (constant, no trend).
    Adf { series: Vec<f64>, max_lag: usize },
    /// Gaussian hidden Markov regime model fitted by Baum-Welch.
    RegimeHmm {
        observations: Vec<f64>,
        n_states: usize,
        max_iter: usize,
        tol: f64,
    },
}

/// The wire names of every op, in declaration order.
pub const NUMERIC_EVALUATE_OPS: [&str; 17] = [
    "rolling_z_score",
    "ewma",
    "information_coefficient",
    "information_ratio",
    "effective_independent_n",
    "diebold_mariano",
    "deflated_sharpe",
    "purged_cpcv",
    "probability_of_backtest_overfit",
    "brier",
    "kalman_level",
    "kalman_beta",
    "kalman_volatility",
    "ou_calibrate",
    "ou_thresholds",
    "adf",
    "regime_hmm",
];

impl NumericEvaluateOp {
    /// The op's wire name.
    pub fn name(&self) -> &'static str {
        match self {
            Self::RollingZScore { .. } => NUMERIC_EVALUATE_OPS[0],
            Self::Ewma { .. } => NUMERIC_EVALUATE_OPS[1],
            Self::InformationCoefficient { .. } => NUMERIC_EVALUATE_OPS[2],
            Self::InformationRatio { .. } => NUMERIC_EVALUATE_OPS[3],
            Self::EffectiveIndependentN { .. } => NUMERIC_EVALUATE_OPS[4],
            Self::DieboldMariano { .. } => NUMERIC_EVALUATE_OPS[5],
            Self::DeflatedSharpe { .. } => NUMERIC_EVALUATE_OPS[6],
            Self::PurgedCpcv { .. } => NUMERIC_EVALUATE_OPS[7],
            Self::ProbabilityOfBacktestOverfit { .. } => NUMERIC_EVALUATE_OPS[8],
            Self::Brier { .. } => NUMERIC_EVALUATE_OPS[9],
            Self::KalmanLevel { .. } => NUMERIC_EVALUATE_OPS[10],
            Self::KalmanBeta { .. } => NUMERIC_EVALUATE_OPS[11],
            Self::KalmanVolatility { .. } => NUMERIC_EVALUATE_OPS[12],
            Self::OuCalibrate { .. } => NUMERIC_EVALUATE_OPS[13],
            Self::OuThresholds { .. } => NUMERIC_EVALUATE_OPS[14],
            Self::Adf { .. } => NUMERIC_EVALUATE_OPS[15],
            Self::RegimeHmm { .. } => NUMERIC_EVALUATE_OPS[16],
        }
    }

    /// Decode an op from its JSON form. A tag that names no op is refused with
    /// `KERNEL_OP_UNKNOWN`; a known op whose fields do not decode is refused with
    /// `KERNEL_PARAM_INVALID`. There is no alias for a retired name.
    pub fn parse(value: &serde_json::Value) -> Result<Self, KernelRefusal> {
        let tag = value
            .get("op")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| KernelRefusal::new(KernelCode::OpUnknown, "the op has no `op` tag"))?;
        if !NUMERIC_EVALUATE_OPS.contains(&tag) {
            return Err(KernelRefusal::new(
                KernelCode::OpUnknown,
                format!("no numeric op named `{tag}`"),
            ));
        }
        serde_json::from_value(value.clone())
            .map_err(|e| KernelRefusal::new(KernelCode::ParamInvalid, e.to_string()))
    }
}

/// One purged combinatorial cross-validation split.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct CvSplit {
    pub train: Vec<usize>,
    pub test: Vec<usize>,
}

/// Diebold-Mariano test of equal predictive accuracy.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct DieboldMariano {
    pub statistic: f64,
    pub p_value: f64,
    pub a_better: bool,
}

/// Filtered states and variances of a scalar Kalman filter.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct KalmanState {
    pub states: Vec<f64>,
    pub variances: Vec<f64>,
}

/// Augmented Dickey-Fuller test result.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct AdfResult {
    pub statistic: f64,
    pub used_lag: usize,
    pub n_obs: usize,
    /// Finite-sample-interpolated MacKinnon critical values (constant, no trend).
    pub crit_1pct: f64,
    pub crit_5pct: f64,
    pub crit_10pct: f64,
    /// Approximate p-value (monotone interpolation across the critical points).
    pub p_value_approx: f64,
    pub stationary_1pct: bool,
    pub stationary_5pct: bool,
    pub stationary_10pct: bool,
}

/// Calibrated Ornstein-Uhlenbeck parameters.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct OuParams {
    pub theta: f64,     // mean-reversion rate
    pub mu: f64,        // long-run mean
    pub sigma: f64,     // instantaneous volatility
    pub half_life: f64, // ln(2)/theta
    pub sigma_eq: f64,  // equilibrium std σ/√(2θ)
}

/// MFPT-optimal Ornstein-Uhlenbeck entry/exit thresholds.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct OuThresholds {
    pub entry_long: f64,  // enter long below this (μ − z·σ_eq)
    pub entry_short: f64, // enter short above this (μ + z·σ_eq)
    pub exit: f64,        // exit at the mean
    pub z: f64,           // optimal entry deviation in σ_eq units
    pub expected_return_per_unit_time: f64,
}

/// A fitted Gaussian HMM regime model.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct RegimeResult {
    /// Most likely state sequence (Viterbi path)
    pub states: Vec<usize>,
    /// State means (emission parameters)
    pub means: Vec<f64>,
    /// State standard deviations
    pub stds: Vec<f64>,
    /// Transition matrix (n_states x n_states)
    pub transition_matrix: Vec<Vec<f64>>,
    /// Initial state probabilities
    pub initial_probs: Vec<f64>,
    /// Log-likelihood of the final model
    pub log_likelihood: f64,
    /// Number of EM iterations until convergence
    pub n_iterations: usize,
}

/// What one op answers. Each variant is the declared result of one or more ops.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum NumericKernelOutput {
    /// One exact `f64`.
    Scalar { value: f64 },
    /// One value per input, `None` where a kernel has not warmed up.
    Series { values: Vec<Option<f64>> },
    /// One value per input.
    Vector { values: Vec<f64> },
    /// Cross-validation splits.
    Splits { splits: Vec<CvSplit> },
    /// Diebold-Mariano result.
    DieboldMariano(DieboldMariano),
    /// Kalman states and variances.
    Kalman(KalmanState),
    /// ADF result.
    Adf(AdfResult),
    /// OU parameters.
    OuParams(OuParams),
    /// OU thresholds.
    OuThresholds(OuThresholds),
    /// HMM regime fit.
    Regime(RegimeResult),
}

/// The stable refusal codes of a numeric kernel evaluation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum KernelCode {
    #[serde(rename = "KERNEL_INPUT_SHORT")]
    InputShort,
    #[serde(rename = "KERNEL_INPUT_NONFINITE")]
    InputNonfinite,
    #[serde(rename = "KERNEL_LENGTH_MISMATCH")]
    LengthMismatch,
    #[serde(rename = "KERNEL_PARAM_INVALID")]
    ParamInvalid,
    #[serde(rename = "KERNEL_SINGULAR")]
    Singular,
    #[serde(rename = "KERNEL_STATE_BUDGET")]
    StateBudget,
    #[serde(rename = "KERNEL_WORK_BUDGET")]
    WorkBudget,
    #[serde(rename = "KERNEL_OP_UNKNOWN")]
    OpUnknown,
}

impl KernelCode {
    /// The code as rendered in an error detail.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::InputShort => "KERNEL_INPUT_SHORT",
            Self::InputNonfinite => "KERNEL_INPUT_NONFINITE",
            Self::LengthMismatch => "KERNEL_LENGTH_MISMATCH",
            Self::ParamInvalid => "KERNEL_PARAM_INVALID",
            Self::Singular => "KERNEL_SINGULAR",
            Self::StateBudget => "KERNEL_STATE_BUDGET",
            Self::WorkBudget => "KERNEL_WORK_BUDGET",
            Self::OpUnknown => "KERNEL_OP_UNKNOWN",
        }
    }
}

/// A refused evaluation: a stable code and what was wrong.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct KernelRefusal {
    pub code: KernelCode,
    pub detail: String,
}

impl KernelRefusal {
    /// A refusal with `code` and `detail`.
    pub fn new(code: KernelCode, detail: impl Into<String>) -> Self {
        Self {
            code,
            detail: detail.into(),
        }
    }
}

impl fmt::Display for KernelRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code.as_str(), self.detail)
    }
}

impl std::error::Error for KernelRefusal {}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-FINANCE-PRIMITIVES-R002.2.5
    #[test]
    fn every_op_name_matches_its_wire_tag() {
        let ops = [
            NumericEvaluateOp::Brier {
                forecasts: vec![],
                outcomes: vec![],
            },
            NumericEvaluateOp::Adf {
                series: vec![],
                max_lag: 1,
            },
            NumericEvaluateOp::OuCalibrate {
                spread: vec![],
                dt: 1.0,
            },
        ];
        for op in ops {
            let json = serde_json::to_value(&op).unwrap();
            assert_eq!(json["op"], op.name());
            assert_eq!(NumericEvaluateOp::parse(&json).unwrap(), op);
        }
    }

    // spec: MEDULLA-FINANCE-PRIMITIVES-R002.2.5
    #[test]
    fn unknown_op_is_refused_with_its_code() {
        let err = NumericEvaluateOp::parse(&serde_json::json!({"op": "finance_var"})).unwrap_err();
        assert_eq!(err.code, KernelCode::OpUnknown);
        assert!(err.to_string().starts_with("KERNEL_OP_UNKNOWN: "));
    }
    /// One representative op per wire tag, in `NUMERIC_EVALUATE_OPS` order.
    fn round_trip_ops() -> Vec<NumericEvaluateOp> {
        let v = |n: usize| {
            (0..n)
                .map(|i| ((i * 7919) % 23) as f64 / 10.0 - 1.1)
                .collect::<Vec<f64>>()
        };
        vec![
            NumericEvaluateOp::RollingZScore {
                values: v(12),
                window: 4,
            },
            NumericEvaluateOp::Ewma {
                values: v(12),
                smoothing: EwmaSmoothing::Span(5.0),
            },
            NumericEvaluateOp::InformationCoefficient {
                signal: v(12),
                forward_returns: v(12).iter().map(|x| x * 0.5 + 0.1).collect(),
            },
            NumericEvaluateOp::InformationRatio {
                ic: 0.05,
                n_independent: 16.0,
            },
            NumericEvaluateOp::EffectiveIndependentN {
                returns_matrix: vec![
                    v(10),
                    v(10).iter().map(|x| -x).collect(),
                    v(10).iter().map(|x| x * 0.3).collect(),
                ],
            },
            NumericEvaluateOp::DieboldMariano {
                losses_a: v(30),
                losses_b: v(30).iter().map(|x| x * x).collect(),
                h: 1,
            },
            NumericEvaluateOp::DeflatedSharpe {
                observed_sr: 1.2,
                n_trials: 20,
                sr_returns: v(40),
            },
            NumericEvaluateOp::PurgedCpcv {
                n_samples: 60,
                n_groups: 6,
                n_test_groups: 2,
                purge_window: 2,
                embargo: 1,
            },
            NumericEvaluateOp::ProbabilityOfBacktestOverfit {
                insample: vec![
                    v(8),
                    v(8).iter().map(|x| x + 0.2).collect(),
                    v(8).iter().map(|x| x * 0.4).collect(),
                    v(8).iter().map(|x| -x).collect(),
                ],
                oos: vec![
                    v(8).iter().map(|x| x - 0.1).collect(),
                    v(8),
                    v(8).iter().map(|x| x * 0.2).collect(),
                    v(8).iter().map(|x| x + 0.3).collect(),
                ],
            },
            NumericEvaluateOp::Brier {
                forecasts: vec![0.1, 0.8, 0.6, 0.3],
                outcomes: vec![0.0, 1.0, 1.0, 0.0],
            },
            NumericEvaluateOp::KalmanLevel {
                observations: v(10),
                f: 1.0,
                q: 0.01,
                h: 1.0,
                r: 0.1,
                x0: 0.0,
                p0: 1.0,
            },
            NumericEvaluateOp::KalmanBeta {
                market_returns: v(20),
                asset_returns: v(20).iter().map(|x| x * 1.2).collect(),
                q: 0.001,
                r: 0.01,
                beta0: 1.0,
                p0: 1.0,
            },
            NumericEvaluateOp::KalmanVolatility {
                returns: v(20),
                q: 0.01,
                r: 0.1,
                log_var0: None,
                p0: 1.0,
                annualization: 252.0,
            },
            NumericEvaluateOp::OuCalibrate {
                spread: v(60),
                dt: 1.0,
            },
            NumericEvaluateOp::OuThresholds {
                theta: 0.5,
                mu: 0.0,
                sigma: 0.2,
                sigma_eq: 0.3,
                cost: 0.01,
            },
            NumericEvaluateOp::Adf {
                series: v(40),
                max_lag: 1,
            },
            NumericEvaluateOp::RegimeHmm {
                observations: v(40),
                n_states: 2,
                max_iter: 20,
                tol: 1e-4,
            },
        ]
    }

    // spec: MEDULLA-FINANCE-PRIMITIVES-R002.3
    #[test]
    fn emit_round_trip_vectors() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/numeric_evaluate_vectors.json");
        let ops = round_trip_ops();
        assert_eq!(ops.len(), NUMERIC_EVALUATE_OPS.len());
        let generated = serde_json::json!({
            "vectors": ops.iter().map(|op| serde_json::to_value(op).unwrap()).collect::<Vec<_>>()
        });
        if std::env::var_os("MEDULLA_EMIT_NUMERIC_VECTORS").is_some() {
            let text = serde_json::to_string_pretty(&generated).expect("vectors serialise");
            std::fs::write(&path, format!("{text}\n")).expect("write fixture");
        }
        let committed: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).expect("fixture present"))
                .expect("fixture is JSON");
        assert_eq!(
            committed, generated,
            "tests/fixtures/numeric_evaluate_vectors.json drifted from the serde types; rerun with MEDULLA_EMIT_NUMERIC_VECTORS=1"
        );
        for (vector, name) in generated["vectors"]
            .as_array()
            .unwrap()
            .iter()
            .zip(NUMERIC_EVALUATE_OPS)
        {
            assert_eq!(vector["op"], name);
            assert_eq!(NumericEvaluateOp::parse(vector).unwrap().name(), name);
        }
    }
}
