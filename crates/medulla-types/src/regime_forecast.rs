//! Wire types of the served regime forecast, calibration gate and sizing input
//! (MEDULLA-FINANCE-PRIMITIVES-R002.11.0 to R002.11.3).
//!
//! `Method::RegimeForecast` answers one [`RegimeForecastResult`]: the multi-step
//! state probabilities `p_h = p0 * T^h` of a pinned regime model, with the per-horizon
//! categorical Brier gate when the request carries held-out samples and a policy.
//! `Method::RegimeSizingInput` turns a passing forecast into an advisory target
//! weight per horizon under a versioned [`RegimeSizingPolicy`]. Nothing here
//! creates an order and no threshold, weight or cap has a default.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// The model node is older or newer than the revision the request pins.
pub const FORECAST_MODEL_STALE: &str = "FORECAST_MODEL_STALE";
/// Horizons are empty, unsorted, not positive or too far.
pub const FORECAST_HORIZON_INVALID: &str = "FORECAST_HORIZON_INVALID";
/// The model matrix, posterior or state ids are not a valid Markov model.
pub const FORECAST_MATRIX_INVALID: &str = "FORECAST_MATRIX_INVALID";
/// The caller supplied `p0` or the transition matrix.
pub const FORECAST_INPUT_SUPPLIED: &str = "FORECAST_INPUT_SUPPLIED";
/// Two held-out rows share an id or one (source, origin, horizon).
pub const CALIBRATION_ROW_DUPLICATE: &str = "CALIBRATION_ROW_DUPLICATE";
/// An origin has no row for a requested horizon.
pub const CALIBRATION_ROW_MISSING: &str = "CALIBRATION_ROW_MISSING";
/// A row names another model revision, source, state order or an unknown state.
pub const CALIBRATION_ROW_MISMATCH: &str = "CALIBRATION_ROW_MISMATCH";
/// A row's origin is not before its target, or its target is after the forecast as-of.
pub const CALIBRATION_FUTURE_LEAK: &str = "CALIBRATION_FUTURE_LEAK";
/// A horizon has fewer held-out rows than the policy demands.
pub const CALIBRATION_SAMPLES_LOW: &str = "CALIBRATION_SAMPLES_LOW";
/// The calibration policy is incomplete or out of range.
pub const CALIBRATION_POLICY_INVALID: &str = "CALIBRATION_POLICY_INVALID";
/// The sizing input abstained; the detail starts with one [`SizingAbstainReason`].
pub const SIZING_ABSTAIN: &str = "SIZING_ABSTAIN";
/// The pinned regime model node is absent or unreadable.
pub const REGIME_MODEL_NOT_FOUND: &str = "REGIME_MODEL_NOT_FOUND";

/// One forecast request. `supplied_p0` and `supplied_transition_matrix` exist only
/// so a caller that sends them gets the typed [`FORECAST_INPUT_SUPPLIED`] refusal.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct RegimeForecastRequest {
    pub model_id: String,
    pub model_revision: String,
    pub source_series_id: String,
    pub source_revision: String,
    /// RFC 3339 instant the posterior `p0` is valid at.
    pub as_of: String,
    /// Strictly increasing positive step counts.
    pub horizons: Vec<u32>,
    pub horizon_unit: String,
    /// Held-out evidence and the policy that gates the forecast; absent means no gate.
    #[serde(default)]
    pub calibration: Option<RegimeCalibrationInput>,
    #[serde(default)]
    pub supplied_p0: Option<Vec<f64>>,
    #[serde(default)]
    pub supplied_transition_matrix: Option<Vec<Vec<f64>>>,
}

/// Held-out samples and the policy they are scored against.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct RegimeCalibrationInput {
    pub policy: RegimeCalibrationPolicy,
    pub samples: Vec<HeldoutSample>,
}

/// One held-out forecast and the state that was realized.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct HeldoutSample {
    pub sample_id: String,
    pub model_revision: String,
    pub source_series_id: String,
    pub origin_as_of: String,
    pub horizon: u32,
    pub ordered_state_ids: Vec<String>,
    pub predicted_probability_vector: Vec<f64>,
    pub realized_state_id: String,
    pub target_observation_at: String,
    pub scoring_revision: String,
}

/// The three thresholds of one horizon; none has a default.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct HorizonThresholds {
    pub horizon: u32,
    /// In `[0, 2]`, squared probability.
    pub max_brier: f64,
    /// In `[0, 1]`.
    pub min_top_state_probability: f64,
    /// Positive.
    pub minimum_heldout_examples: u64,
}

/// A complete versioned calibration policy: every requested horizon appears once.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct RegimeCalibrationPolicy {
    pub policy_id: String,
    pub revision: String,
    pub horizons: Vec<HorizonThresholds>,
}

/// The model and source revisions a forecast rests on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct RegimeRevisions {
    pub model_revision: String,
    pub source_revision: String,
}

/// The gate outcome of one horizon.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct HorizonCalibration {
    pub horizon: u32,
    pub dataset_digest: String,
    pub model_revision: String,
    pub state_ids_digest: String,
    pub scoring_revision: String,
    pub calibration_policy_digest: String,
    pub sample_count: u64,
    pub brier: f64,
    pub applied_thresholds: HorizonThresholds,
    pub passed: bool,
}

/// The served forecast.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct RegimeForecastResult {
    pub forecast_digest: String,
    pub revisions: RegimeRevisions,
    pub as_of: String,
    pub horizon_unit: String,
    pub horizons: Vec<u32>,
    pub ordered_state_ids: Vec<String>,
    pub posterior_p0: Vec<f64>,
    pub transition_matrix_digest: String,
    pub per_horizon_probability_vectors: Vec<Vec<f64>>,
    pub per_horizon_top_state_probability: Vec<f64>,
    /// One entry per requested horizon when the request carried calibration; else empty.
    pub calibration: Vec<HorizonCalibration>,
}

/// A versioned sizing policy. Weights and the cap are fixed-point integers in `unit`
/// at `fixed_point_scale` ticks per whole unit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct RegimeSizingPolicy {
    pub policy_id: String,
    pub revision: String,
    pub instrument_id: String,
    pub target_weight_by_state: BTreeMap<String, i64>,
    pub unit: String,
    pub fixed_point_scale: i64,
    pub risk_cap: i64,
}

/// Why a sizing input abstained.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SizingAbstainReason {
    PolicyWeightMissing,
    RiskCapExceeded,
    ForecastStale,
    CalibrationFailed,
}

impl SizingAbstainReason {
    /// The closed set, in check order.
    pub const ALL: [Self; 4] = [
        Self::ForecastStale,
        Self::CalibrationFailed,
        Self::PolicyWeightMissing,
        Self::RiskCapExceeded,
    ];

    /// The stable reason code.
    pub fn code(self) -> &'static str {
        match self {
            Self::PolicyWeightMissing => "POLICY_WEIGHT_MISSING",
            Self::RiskCapExceeded => "RISK_CAP_EXCEEDED",
            Self::ForecastStale => "FORECAST_STALE",
            Self::CalibrationFailed => "CALIBRATION_FAILED",
        }
    }
}

/// One horizon's advisory target weight, in ticks of the policy scale.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct HorizonSizing {
    pub horizon: u32,
    pub expected_target_weight: i64,
}

/// The served sizing input: advisory, never an order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct RegimeSizingResult {
    pub forecast_digest: String,
    pub sizing_policy_digest: String,
    pub instrument_id: String,
    pub unit: String,
    pub fixed_point_scale: i64,
    pub per_horizon: Vec<HorizonSizing>,
}
