//! The `decision.rank.*` knobs (MEDULLA-DECISION-ENGINE-R829.2): the weights of
//! the capability-ranking features medulla computes from candidate text. Each
//! is a non-negative weight stored as micros.

use crate::decision::knobs::{KnobSafety, KnobSpec, KnobValue};
use crate::decision::policy::DecisionPolicy;

const fn weight(name: &'static str, default_micros: i64) -> KnobSpec {
    KnobSpec {
        name,
        unit: "weight",
        default: KnobValue::DecimalMicros(default_micros),
        min: KnobValue::DecimalMicros(0),
        max: KnobValue::DecimalMicros(10_000_000),
        safety: KnobSafety::Tunable,
    }
}

/// Weight of a name match in the rarity-weighted overlap (starting value 2.0).
pub const NAME_WEIGHT: KnobSpec = weight("decision.rank.name_weight", 2_000_000);
/// Weight of the semantic similarity (starting value 1.0).
pub const SEMANTIC_WEIGHT: KnobSpec = weight("decision.rank.semantic_weight", 1_000_000);

/// Every `decision.rank.*` knob, each `Tunable`.
pub const KNOBS: &[KnobSpec] = &[NAME_WEIGHT, SEMANTIC_WEIGHT];

/// The weights the pinned policy sets as `(name, semantic)`: each knob, or its
/// default.
pub fn weights(policy: &DecisionPolicy) -> (f64, f64) {
    let read = |spec: &KnobSpec| {
        let micros = match policy.knob(spec) {
            KnobValue::DecimalMicros(micros) => micros.clamp(0, 10_000_000),
            _ => match spec.default {
                KnobValue::DecimalMicros(micros) => micros,
                _ => 0,
            },
        };
        micros as f64 / 1_000_000.0
    };
    (read(&NAME_WEIGHT), read(&SEMANTIC_WEIGHT))
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-DECISION-ENGINE-R829.2
    #[test]
    fn the_rank_weights_are_declared_with_the_stated_starting_values() {
        assert_eq!(
            KNOBS.iter().map(|k| k.name).collect::<Vec<_>>(),
            ["decision.rank.name_weight", "decision.rank.semantic_weight"]
        );
        assert_eq!(NAME_WEIGHT.default, KnobValue::DecimalMicros(2_000_000));
        assert_eq!(SEMANTIC_WEIGHT.default, KnobValue::DecimalMicros(1_000_000));
    }
}
