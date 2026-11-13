//! Delegation admission over a stable outcome window
//! (MEDULLA-DECISION-ENGINE-R134.102).
//!
//! Types, refusals and knobs only; the pure verdict is `admit_delegation` in
//! `medulla-decision-stats`.

use serde::{Deserialize, Serialize};

use super::knobs::{KnobSafety, KnobSpec, KnobValue};
use super::policy::DecisionPolicy;
use super::record::outcome::{DecisionRefusal, RefusalCode};

/// The window holds fewer counted outcomes than `delegation.min_samples`.
pub const DELEGATION_INSUFFICIENT_SAMPLES: RefusalCode =
    RefusalCode::new("DELEGATION_INSUFFICIENT_SAMPLES");
/// The pooled success rate is under the threshold.
pub const DELEGATION_BELOW_THRESHOLD: RefusalCode = RefusalCode::new("DELEGATION_BELOW_THRESHOLD");
/// The window ends after the decision clock.
pub const DELEGATION_WINDOW_OPEN: RefusalCode = RefusalCode::new("DELEGATION_WINDOW_OPEN");
/// The counted records were decided under more than one policy digest.
pub const DELEGATION_MIXED_REVISIONS: RefusalCode = RefusalCode::new("DELEGATION_MIXED_REVISIONS");
/// The newest outcome is older than `delegation.max_age`.
pub const DELEGATION_STALE_EVIDENCE: RefusalCode = RefusalCode::new("DELEGATION_STALE_EVIDENCE");

/// The `REFUSALS` table of the family, registered in `codes::FAMILY_TABLES`.
pub const REFUSALS: &[RefusalCode] = &[
    DELEGATION_INSUFFICIENT_SAMPLES,
    DELEGATION_BELOW_THRESHOLD,
    DELEGATION_WINDOW_OPEN,
    DELEGATION_MIXED_REVISIONS,
    DELEGATION_STALE_EVIDENCE,
];

/// The only metric the decision log records an outcome for.
pub const METRIC_SUCCESS: &str = "success";

/// One fixed-point unit of `threshold_q32`.
pub const Q32_ONE: u64 = 1 << 32;

/// What to admit: a task class, a metric, a threshold that may only raise the
/// knob, and the closed window the evidence comes from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct DelegationAdmissionRequest {
    /// The logged question id the executed records of the class carry.
    pub task_class: String,
    pub metric_id: String,
    /// A rate in Q32 (`1 << 32` is 1.0). Zero asks for no raise.
    pub threshold_q32: u64,
    pub window_start_ms: u64,
    pub window_end_ms: u64,
}

/// One independent evaluation of an executed record, as admission counts it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct DelegationOutcome {
    pub record_id: String,
    pub policy_digest: String,
    pub evaluation_id: String,
    pub success: bool,
    pub recorded_at_ms: u64,
}

/// The resolved `delegation.*` knobs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DelegationKnobs {
    pub min_samples: u64,
    pub max_age_ms: u64,
    pub quality_threshold_micros: u64,
}

/// The verdict of an admission: everything it was bound to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct DelegationVerdict {
    pub task_class: String,
    pub metric_id: String,
    pub threshold_micros: u64,
    pub window_start_ms: u64,
    pub window_end_ms: u64,
    pub samples: u64,
    pub successes: u64,
    pub outcome_set_digest: String,
    pub policy_digest: String,
    pub verdict_digest: String,
}

/// Why delegation was not admitted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DelegationRefusal {
    InsufficientSamples {
        samples: u64,
        needed: u64,
    },
    BelowThreshold {
        successes: u64,
        samples: u64,
        threshold_micros: u64,
    },
    WindowOpen {
        window_end_ms: u64,
        now_ms: u64,
    },
    MixedRevisions {
        revisions: usize,
    },
    StaleEvidence {
        newest_ms: u64,
        max_age_ms: u64,
    },
}

impl DecisionRefusal for DelegationRefusal {
    fn code(&self) -> RefusalCode {
        match self {
            Self::InsufficientSamples { .. } => DELEGATION_INSUFFICIENT_SAMPLES,
            Self::BelowThreshold { .. } => DELEGATION_BELOW_THRESHOLD,
            Self::WindowOpen { .. } => DELEGATION_WINDOW_OPEN,
            Self::MixedRevisions { .. } => DELEGATION_MIXED_REVISIONS,
            Self::StaleEvidence { .. } => DELEGATION_STALE_EVIDENCE,
        }
    }

    fn detail(&self) -> String {
        match self {
            Self::InsufficientSamples { samples, needed } => {
                format!("{samples} counted outcomes, {needed} needed")
            }
            Self::BelowThreshold {
                successes,
                samples,
                threshold_micros,
            } => format!("{successes}/{samples} below threshold {threshold_micros} micros"),
            Self::WindowOpen {
                window_end_ms,
                now_ms,
            } => format!("window ends at {window_end_ms}, after the clock {now_ms}"),
            Self::MixedRevisions { revisions } => format!("{revisions} policy revisions"),
            Self::StaleEvidence {
                newest_ms,
                max_age_ms,
            } => format!("newest outcome {newest_ms}, older than {max_age_ms} ms"),
        }
    }
}

/// The `delegation.*` knobs (MEDULLA-DECISION-ENGINE-R134.102). Decision-plane
/// thresholds, pinned through the decision policy (DE-R059.1-KNOBS).
pub const KNOBS: &[KnobSpec] = &[
    KnobSpec {
        name: "delegation.min_samples",
        unit: "count",
        default: KnobValue::Integer(30),
        min: KnobValue::Integer(5),
        max: KnobValue::Integer(10_000),
        safety: KnobSafety::Tunable,
    },
    KnobSpec {
        name: "delegation.max_age",
        unit: "ms",
        default: KnobValue::DurationMs(604_800_000),
        min: KnobValue::DurationMs(3_600_000),
        max: KnobValue::DurationMs(7_776_000_000),
        safety: KnobSafety::Tunable,
    },
    KnobSpec {
        name: "delegation.quality_threshold",
        unit: "fraction",
        default: KnobValue::DecimalMicros(800_000),
        min: KnobValue::DecimalMicros(0),
        max: KnobValue::DecimalMicros(1_000_000),
        safety: KnobSafety::Tunable,
    },
];

/// The resolved `delegation.*` knobs under `policy`.
pub fn knob_values(policy: &DecisionPolicy) -> DelegationKnobs {
    let number = |index: usize| match policy.knob(&KNOBS[index]) {
        KnobValue::DurationMs(v) => v,
        KnobValue::Integer(v) | KnobValue::DecimalMicros(v) => u64::try_from(v).unwrap_or(0),
    };
    DelegationKnobs {
        min_samples: number(0),
        max_age_ms: number(1),
        quality_threshold_micros: number(2),
    }
}

/// SHA-256 (64 lowercase hex characters) over `parts`, each length-prefixed so
/// no two part lists share a preimage.
pub fn digest_parts(parts: &[&str]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update((part.len() as u64).to_be_bytes());
        hasher.update(part.as_bytes());
    }
    hex::encode(hasher.finalize())
}
