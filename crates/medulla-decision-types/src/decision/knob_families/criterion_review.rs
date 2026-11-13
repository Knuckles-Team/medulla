//! The `criterion_review.*` knobs (MEDULLA-DECISION-ENGINE-R136.103): how many
//! recent runs the judge-versus-outcome divergence reads, and how far the
//! judge's score trend may run ahead of the independent outcome trend.

use crate::decision::knobs::{KnobSafety, KnobSpec, KnobValue};
use crate::decision::policy::DecisionPolicy;

/// The `criterion_review.*` knobs, each `Tunable`.
pub const KNOBS: &[KnobSpec] = &[
    KnobSpec {
        name: "criterion_review.divergence_window",
        unit: "runs",
        default: KnobValue::Integer(20),
        min: KnobValue::Integer(5),
        max: KnobValue::Integer(200),
        safety: KnobSafety::Tunable,
    },
    KnobSpec {
        name: "criterion_review.divergence_threshold",
        unit: "score",
        default: KnobValue::DecimalMicros(150_000),
        min: KnobValue::DecimalMicros(10_000),
        max: KnobValue::DecimalMicros(1_000_000),
        safety: KnobSafety::Tunable,
    },
];

/// The number of recent runs the divergence reads.
pub fn divergence_window(policy: &DecisionPolicy) -> usize {
    match policy.knob(&KNOBS[0]) {
        KnobValue::Integer(v) => usize::try_from(v).unwrap_or(20),
        _ => 20,
    }
}

/// The divergence above which the evidence refuses, in millionths of a score
/// unit.
pub fn divergence_threshold_micros(policy: &DecisionPolicy) -> i64 {
    match policy.knob(&KNOBS[1]) {
        KnobValue::DecimalMicros(v) => v,
        _ => 150_000,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decision::knobs::KnobName;

    // spec: MEDULLA-DECISION-ENGINE-R136.103
    #[test]
    fn knobs_read_defaults_and_pinned_values() {
        let default = DecisionPolicy::engine_default();
        assert_eq!(divergence_window(&default), 20);
        assert_eq!(divergence_threshold_micros(&default), 150_000);
        let mut pinned = DecisionPolicy::engine_default();
        pinned.knobs.insert(
            KnobName::new("criterion_review.divergence_window"),
            KnobValue::Integer(8),
        );
        pinned.knobs.insert(
            KnobName::new("criterion_review.divergence_threshold"),
            KnobValue::DecimalMicros(40_000),
        );
        assert_eq!(divergence_window(&pinned), 8);
        assert_eq!(divergence_threshold_micros(&pinned), 40_000);
        for spec in KNOBS {
            assert!(spec.admits(&spec.default));
        }
    }
}
