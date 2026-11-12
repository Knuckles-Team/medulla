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
}
