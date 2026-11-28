//! Safety bound types (MEDULLA-DECISION-ENGINE-R001.100.1).
//!
//! A [`SafetyBound`] is one pinned limit on a measured metric. A
//! [`SafetyMeasurement`] is the caller's claim about the current value of that
//! metric. The evaluator (in the stats crate) compares the two and returns one
//! [`BoundCheck`] per bound plus a single [`SafetyVerdict`]. Every value is an
//! `i64` on the `Q32` scale, so a non-finite value cannot be expressed.

use serde::{Deserialize, Serialize};

use crate::decision::knobs::{KnobSafety, KnobSpec, KnobValue};
use medulla_types::refusal_code::RefusalCode;

/// Default of `knob safety.max_measurement_age`, in milliseconds (3600 s).
pub const DEFAULT_MAX_MEASUREMENT_AGE_MS: u64 = 3_600_000;

/// The `safety.*` knobs: the oldest measurement a bound check will accept.
/// `Fixed`: a knob-set revision may never loosen a safety check.
pub const KNOBS: &[KnobSpec] = &[KnobSpec {
    name: "safety.max_measurement_age",
    unit: "ms",
    default: KnobValue::DurationMs(DEFAULT_MAX_MEASUREMENT_AGE_MS),
    min: KnobValue::DurationMs(1_000),
    max: KnobValue::DurationMs(86_400_000),
    safety: KnobSafety::Fixed,
}];

/// How a measured value must compare to the limit to be inside the bound.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum BoundComparison {
    /// `value < limit`
    Lt,
    /// `value <= limit`
    Le,
    /// `value > limit`
    Gt,
    /// `value >= limit`
    Ge,
}

impl BoundComparison {
    /// Whether `value` satisfies the bound against `limit`.
    pub const fn holds(self, value: i64, limit: i64) -> bool {
        match self {
            Self::Lt => value < limit,
            Self::Le => value <= limit,
            Self::Gt => value > limit,
            Self::Ge => value >= limit,
        }
    }
}

/// One pinned safety limit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct SafetyBound {
    pub bound_id: String,
    pub revision: u64,
    pub digest: String,
    pub metric_id: String,
    pub unit: String,
    pub comparison: BoundComparison,
    pub limit_q32: i64,
}

/// A caller claim about the current value of a bound's metric.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct SafetyMeasurement {
    pub bound_id: String,
    pub value_q32: i64,
    pub unit: String,
    pub source_revision: u64,
    pub measured_at_ms: u64,
}

/// Why one bound check came out as it did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum BoundOutcome {
    /// The measurement is fresh, in the bound's unit and inside the limit.
    Within,
    /// The measurement is fresh and in unit but outside the limit.
    Exceeded,
    /// No measurement names this bound.
    MeasurementMissing,
    /// The measurement is older than `knob safety.max_measurement_age`.
    MeasurementStale,
    /// The measurement's unit is not the bound's unit.
    UnitMismatch,
}

/// The measurement is fresh and in unit but outside the limit.
pub const SAFETY_BOUND_EXCEEDED: RefusalCode = RefusalCode::new("SAFETY_BOUND_EXCEEDED");
/// No measurement names the bound (or the request carries none at all).
pub const SAFETY_MEASUREMENT_MISSING: RefusalCode = RefusalCode::new("SAFETY_MEASUREMENT_MISSING");
/// The measurement is older than `knob safety.max_measurement_age`.
pub const SAFETY_MEASUREMENT_STALE: RefusalCode = RefusalCode::new("SAFETY_MEASUREMENT_STALE");
/// The measurement's unit is not the bound's unit.
pub const SAFETY_UNIT_MISMATCH: RefusalCode = RefusalCode::new("SAFETY_UNIT_MISMATCH");

/// The `safety` table of registered refusal codes (MEDULLA-DECISION-ENGINE-R010.100).
pub const REFUSALS: &[RefusalCode] = &[
    SAFETY_BOUND_EXCEEDED,
    SAFETY_MEASUREMENT_MISSING,
    SAFETY_MEASUREMENT_STALE,
    SAFETY_UNIT_MISMATCH,
];

impl BoundOutcome {
    /// The typed refusal code the served stage reports for a violated bound.
    pub const fn code(self) -> Option<RefusalCode> {
        match self {
            Self::Within => None,
            Self::Exceeded => Some(SAFETY_BOUND_EXCEEDED),
            Self::MeasurementMissing => Some(SAFETY_MEASUREMENT_MISSING),
            Self::MeasurementStale => Some(SAFETY_MEASUREMENT_STALE),
            Self::UnitMismatch => Some(SAFETY_UNIT_MISMATCH),
        }
    }

    /// The code's wire string, for callers that only render it.
    pub const fn refusal_code(self) -> Option<&'static str> {
        match self.code() {
            Some(code) => Some(code.as_str()),
            None => None,
        }
    }
}

/// The result of checking one bound.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct BoundCheck {
    pub bound_id: String,
    pub outcome: BoundOutcome,
    pub limit_q32: i64,
    pub measured_q32: Option<i64>,
}

/// The verdict over every bound: pass, or the first violated bound in
/// `bound_id` order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "verdict", rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum SafetyVerdict {
    Pass,
    Withheld {
        bound_id: String,
        outcome: BoundOutcome,
        limit_q32: i64,
        measured_q32: Option<i64>,
    },
}
