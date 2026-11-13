//! The `resolve_entity.*` knobs (MEDULLA-DECISION-ENGINE-R032.2.2, R032.2.3):
//! the miscoverage an entity-pair decision may act at, and the smallest
//! gold-labelled set whose coverage is believed. Both are decision-plane
//! thresholds, pinned through the decision policy and sealed with its digest.

use crate::decision::knobs::{KnobSafety, KnobSpec, KnobValue};
use crate::decision::policy::DecisionPolicy;

/// Largest miscoverage the pinned head may be calibrated at for the decision
/// to act on a pair (a fraction, in micros).
pub const ALPHA: KnobSpec = KnobSpec {
    name: "resolve_entity.alpha",
    unit: "ratio",
    default: KnobValue::DecimalMicros(100_000),
    min: KnobValue::DecimalMicros(10_000),
    max: KnobValue::DecimalMicros(500_000),
    safety: KnobSafety::Tunable,
};

/// Fewest gold-labelled pairs an evaluation needs before its coverage counts.
pub const GOLD_MIN_PAIRS: KnobSpec = KnobSpec {
    name: "resolve_entity.gold_min_pairs",
    unit: "pairs",
    default: KnobValue::Integer(200),
    min: KnobValue::Integer(50),
    max: KnobValue::Integer(100_000),
    safety: KnobSafety::Tunable,
};

/// Every `resolve_entity.*` knob, each `Tunable`.
pub const KNOBS: &[KnobSpec] = &[ALPHA, GOLD_MIN_PAIRS];

/// The miscoverage bound the pinned policy sets, in micros: the knob, or its default.
pub fn alpha_micros(policy: &DecisionPolicy) -> i64 {
    match policy.knob(&ALPHA) {
        KnobValue::DecimalMicros(micros) => micros.clamp(10_000, 500_000),
        _ => 100_000,
    }
}

/// The gold-set floor the pinned policy sets: the knob, or its default.
pub fn gold_min_pairs(policy: &DecisionPolicy) -> usize {
    match policy.knob(&GOLD_MIN_PAIRS) {
        KnobValue::Integer(pairs) => pairs.clamp(50, 100_000) as usize,
        _ => 200,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-DECISION-ENGINE-R032.2.2
    #[test]
    fn the_resolve_entity_knobs_are_declared_inside_their_bounds() {
        for spec in KNOBS {
            assert!(spec.admits(&spec.default));
        }
        assert!(!ALPHA.admits(&KnobValue::DecimalMicros(500_001)));
        assert!(!ALPHA.admits(&KnobValue::DecimalMicros(9_999)));
        assert!(!GOLD_MIN_PAIRS.admits(&KnobValue::Integer(49)));
        let policy = DecisionPolicy::engine_default();
        assert_eq!(alpha_micros(&policy), 100_000);
        assert_eq!(gold_min_pairs(&policy), 200);
    }
}
