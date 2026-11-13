//! The `grounded.*` knobs (MEDULLA-DECISION-ENGINE-R100.101.2).

use crate::decision::knobs::{KnobSafety, KnobSpec, KnobValue};
use crate::decision::DecisionPolicy;

/// Index of `grounded.score_digits` in [`KNOBS`].
pub const SCORE_DIGITS: usize = 0;
/// Index of `grounded.min_topic_support` in [`KNOBS`].
pub const MIN_TOPIC_SUPPORT: usize = 1;
/// Index of `grounded.min_valid_votes` in [`KNOBS`].
pub const MIN_VALID_VOTES: usize = 2;

/// Every `grounded.*` knob declared here. The rounding mode of the score is
/// half to even and is not a knob.
pub const KNOBS: &[KnobSpec] = &[
    KnobSpec {
        name: "grounded.score_digits",
        unit: "decimal digits",
        default: KnobValue::Integer(6),
        min: KnobValue::Integer(2),
        max: KnobValue::Integer(9),
        safety: KnobSafety::Tunable,
    },
    KnobSpec {
        name: "grounded.min_topic_support",
        unit: "ratio",
        default: KnobValue::DecimalMicros(500_000),
        min: KnobValue::DecimalMicros(500_000),
        max: KnobValue::DecimalMicros(1_000_000),
        safety: KnobSafety::Tunable,
    },
    KnobSpec {
        name: "grounded.min_valid_votes",
        unit: "votes",
        default: KnobValue::Integer(3),
        min: KnobValue::Integer(1),
        max: KnobValue::Integer(25),
        safety: KnobSafety::Tunable,
    },
];

/// The topic-coverage knobs the pinned policy sets, or their defaults:
/// `grounded.min_topic_support` as an exact rational over one million.
pub fn coverage_knobs(
    policy: &DecisionPolicy,
) -> crate::decision::statistical::grounded_eval::GroundedKnobs {
    let defaults = crate::decision::statistical::grounded_eval::GroundedKnobs::default();
    let min_topic_support = match policy.knob(&KNOBS[MIN_TOPIC_SUPPORT]) {
        KnobValue::DecimalMicros(micros) => u64::try_from(micros)
            .ok()
            .and_then(|micros| {
                crate::decision::numeric::UnitRationalWire::new(micros, 1_000_000).ok()
            })
            .unwrap_or(defaults.min_topic_support),
        _ => defaults.min_topic_support,
    };
    let min_valid_votes = match policy.knob(&KNOBS[MIN_VALID_VOTES]) {
        KnobValue::Integer(votes) => u32::try_from(votes).unwrap_or(defaults.min_valid_votes),
        _ => defaults.min_valid_votes,
    };
    crate::decision::statistical::grounded_eval::GroundedKnobs {
        min_topic_support,
        min_valid_votes,
    }
}

/// The decimal digits a grounded score is rounded to: the pinned value, or
/// the default.
pub fn score_digits(policy: &DecisionPolicy) -> u32 {
    match policy.knob(&KNOBS[SCORE_DIGITS]) {
        KnobValue::Integer(digits) => u32::try_from(digits).unwrap_or(6),
        _ => 6,
    }
}
