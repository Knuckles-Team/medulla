//! The `decision.rank.*` knobs (MEDULLA-DECISION-ENGINE-R829.2): the weights of
//! the capability-ranking features medulla computes from candidate text. Each
//! is a non-negative weight stored as micros.

use super::knobs::{KnobSafety, KnobSpec, KnobValue};
use super::policy::DecisionPolicy;

/// The largest weight a policy may set: 16.0.
const MAX_MICROS: i64 = 16_000_000;

const fn weight(name: &'static str, default_micros: i64) -> KnobSpec {
    KnobSpec {
        name,
        unit: "weight",
        default: KnobValue::DecimalMicros(default_micros),
        min: KnobValue::DecimalMicros(0),
        max: KnobValue::DecimalMicros(MAX_MICROS),
        safety: KnobSafety::Tunable,
    }
}

/// Weight of a name match in the rarity-weighted overlap (starting value 2.0).
pub const NAME_WEIGHT: KnobSpec = weight("decision.rank.name_weight", 2_000_000);
/// Weight of the semantic similarity (starting value 1.0).
pub const SEMANTIC_WEIGHT: KnobSpec = weight("decision.rank.semantic_weight", 1_000_000);

/// Every `decision.rank.*` knob, each `Tunable`.
pub const KNOBS: &[KnobSpec] = &[NAME_WEIGHT, SEMANTIC_WEIGHT];

fn weight_of(policy: &DecisionPolicy, spec: &KnobSpec) -> f64 {
    let micros = match policy.knob(spec) {
        KnobValue::DecimalMicros(micros) => micros.clamp(0, MAX_MICROS),
        _ => match spec.default {
            KnobValue::DecimalMicros(micros) => micros,
            _ => 0,
        },
    };
    micros as f64 / 1_000_000.0
}

/// The resolved `decision.rank.name_weight`: micros divided by 1,000,000.
pub fn name_weight(policy: &DecisionPolicy) -> f64 {
    weight_of(policy, &NAME_WEIGHT)
}

/// The resolved `decision.rank.semantic_weight`: micros divided by 1,000,000.
pub fn semantic_weight(policy: &DecisionPolicy) -> f64 {
    weight_of(policy, &SEMANTIC_WEIGHT)
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-DECISION-ENGINE-R829.2
    #[test]
    fn knob_defaults_and_bounds() {
        assert_eq!(
            KNOBS.iter().map(|k| k.name).collect::<Vec<_>>(),
            ["decision.rank.name_weight", "decision.rank.semantic_weight"]
        );
        assert_eq!(NAME_WEIGHT.default, KnobValue::DecimalMicros(2_000_000));
        assert_eq!(SEMANTIC_WEIGHT.default, KnobValue::DecimalMicros(1_000_000));
        for spec in KNOBS {
            assert_eq!(spec.min, KnobValue::DecimalMicros(0));
            assert_eq!(spec.max, KnobValue::DecimalMicros(16_000_000));
            assert_eq!(spec.unit, "weight");
            assert_eq!(spec.safety, KnobSafety::Tunable);
        }
    }
}
