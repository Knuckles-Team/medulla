//! The `task_plan.*` and `skill_plan.*` knobs (MEDULLA-DECISION-ENGINE-R134.3,
//! R134.4). They are decision-policy knobs, pinned through the decision policy
//! and sealed with its digest.

use crate::decision::knobs::{KnobSafety, KnobSpec, KnobValue};

/// The lowest `Q32` specialist score, as a fraction, that qualifies.
pub const SPECIALIST_MIN_SCORE: KnobSpec = KnobSpec {
    name: "task_plan.specialist_min_score",
    unit: "fraction",
    default: KnobValue::DecimalMicros(200_000),
    min: KnobValue::DecimalMicros(0),
    max: KnobValue::DecimalMicros(1_000_000),
    safety: KnobSafety::Tunable,
};

/// The BM25 score at which relevance reaches one half.
pub const SKILL_BM25_SATURATION: KnobSpec = KnobSpec {
    name: "skill_plan.bm25_saturation",
    unit: "bm25",
    default: KnobValue::DecimalMicros(2_000_000),
    min: KnobValue::DecimalMicros(100_000),
    max: KnobValue::DecimalMicros(20_000_000),
    safety: KnobSafety::Tunable,
};

/// The lowest relevance at which a skill covers a subtask.
pub const SKILL_RELEVANCE_FLOOR: KnobSpec = KnobSpec {
    name: "skill_plan.relevance_floor",
    unit: "fraction",
    default: KnobValue::DecimalMicros(200_000),
    min: KnobValue::DecimalMicros(0),
    max: KnobValue::DecimalMicros(1_000_000),
    safety: KnobSafety::Tunable,
};

/// The prompt size at which subtasks stop merging and a subtask splits.
pub const SKILL_SPLIT_PROMPT_BYTES: KnobSpec = KnobSpec {
    name: "skill_plan.split_prompt_bytes",
    unit: "bytes",
    default: KnobValue::Integer(4096),
    min: KnobValue::Integer(256),
    max: KnobValue::Integer(16384),
    safety: KnobSafety::Tunable,
};

/// Every `task_plan.*` and `skill_plan.*` knob, each `Tunable`.
pub const KNOBS: &[KnobSpec] = &[
    SPECIALIST_MIN_SCORE,
    SKILL_BM25_SATURATION,
    SKILL_RELEVANCE_FLOOR,
    SKILL_SPLIT_PROMPT_BYTES,
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decision::policy::DecisionPolicy;

    // spec: MEDULLA-DECISION-ENGINE-R134.3
    #[test]
    fn defaults_are_inside_bounds_and_a_pinned_knob_moves_the_floor() {
        for spec in KNOBS {
            assert!(spec.admits(&spec.default), "{}", spec.name);
            assert_eq!(spec.safety, KnobSafety::Tunable);
        }
        let mut policy = DecisionPolicy::engine_default();
        assert_eq!(
            policy.knob(&SPECIALIST_MIN_SCORE),
            KnobValue::DecimalMicros(200_000)
        );
        policy.knobs.insert(
            SPECIALIST_MIN_SCORE.knob_name(),
            KnobValue::DecimalMicros(900_000),
        );
        assert_eq!(
            policy.knob(&SPECIALIST_MIN_SCORE),
            KnobValue::DecimalMicros(900_000)
        );
    }
}
