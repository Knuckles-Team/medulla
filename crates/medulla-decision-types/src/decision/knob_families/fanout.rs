//! The `fanout.*` knobs (MEDULLA-DECISION-ENGINE-R134.10). They are
//! decision-policy knobs, pinned through the decision policy and sealed with
//! its digest.

use crate::decision::knobs::{KnobSafety, KnobSpec, KnobValue};
use crate::decision::policy::DecisionPolicy;

/// The most tokens one child summary may hold.
pub const MAX_CHILD_SUMMARY_TOKENS: KnobSpec = KnobSpec {
    name: "fanout.max_child_summary_tokens",
    unit: "tokens",
    default: KnobValue::Integer(2048),
    min: KnobValue::Integer(32),
    max: KnobValue::Integer(16_384),
    safety: KnobSafety::Tunable,
};

/// The summary token budget when the request names none.
pub const MAX_SUMMARY_TOKENS: KnobSpec = KnobSpec {
    name: "fanout.max_summary_tokens",
    unit: "tokens",
    default: KnobValue::Integer(4096),
    min: KnobValue::Integer(64),
    max: KnobValue::Integer(65_536),
    safety: KnobSafety::Tunable,
};

/// Every `fanout.*` knob, each `Tunable`.
pub const KNOBS: &[KnobSpec] = &[MAX_CHILD_SUMMARY_TOKENS, MAX_SUMMARY_TOKENS];

/// The resolved `fanout.*` knobs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FanoutKnobs {
    pub max_child_summary_tokens: u64,
    pub max_summary_tokens: u64,
}

/// The `fanout.*` knobs under `policy`.
pub fn knob_values(policy: &DecisionPolicy) -> FanoutKnobs {
    let number = |spec: &KnobSpec| match policy.knob(spec) {
        KnobValue::Integer(v) | KnobValue::DecimalMicros(v) => u64::try_from(v).unwrap_or(0),
        KnobValue::DurationMs(v) => v,
    };
    FanoutKnobs {
        max_child_summary_tokens: number(&MAX_CHILD_SUMMARY_TOKENS),
        max_summary_tokens: number(&MAX_SUMMARY_TOKENS),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-DECISION-ENGINE-R134.10
    #[test]
    fn defaults_are_inside_bounds_and_a_pinned_knob_moves_the_limit() {
        for spec in KNOBS {
            assert!(spec.admits(&spec.default), "{}", spec.name);
        }
        let mut policy = DecisionPolicy::engine_default();
        assert_eq!(knob_values(&policy).max_child_summary_tokens, 2048);
        policy
            .knobs
            .insert(MAX_CHILD_SUMMARY_TOKENS.knob_name(), KnobValue::Integer(64));
        assert_eq!(knob_values(&policy).max_child_summary_tokens, 64);
    }
}
