//! The `round_verdict.*` knobs (MEDULLA-DECISION-ENGINE-R136.4). They fill the
//! evolution defaults a round verdict is judged against.

use crate::decision::knobs::{KnobSafety, KnobSpec, KnobValue};
use crate::decision::policy::DecisionPolicy;

/// The `round_verdict.*` knobs, each `Tunable`.
pub const KNOBS: &[KnobSpec] = &[
    KnobSpec {
        name: "round_verdict.flip_fraction",
        unit: "fraction",
        default: KnobValue::DecimalMicros(500_000),
        min: KnobValue::DecimalMicros(500_000),
        max: KnobValue::DecimalMicros(1_000_000),
        safety: KnobSafety::Tunable,
    },
    KnobSpec {
        name: "round_verdict.rounds",
        unit: "rounds",
        default: KnobValue::Integer(1),
        min: KnobValue::Integer(1),
        max: KnobValue::Integer(10),
        safety: KnobSafety::Tunable,
    },
];

/// The flip fraction of `policy` as a numerator over one million.
pub fn flip_fraction_micros(policy: &DecisionPolicy) -> u64 {
    match policy.knob(&KNOBS[0]) {
        KnobValue::DecimalMicros(v) => u64::try_from(v).unwrap_or(500_000),
        _ => 500_000,
    }
}

/// The number of rounds of `policy`.
pub fn rounds(policy: &DecisionPolicy) -> u32 {
    match policy.knob(&KNOBS[1]) {
        KnobValue::Integer(v) => u32::try_from(v).unwrap_or(1),
        _ => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decision::knobs::KnobName;

    // spec: MEDULLA-DECISION-ENGINE-R136.4
    #[test]
    fn knobs_read_defaults_and_pinned_values() {
        let default = DecisionPolicy::engine_default();
        assert_eq!(flip_fraction_micros(&default), 500_000);
        assert_eq!(rounds(&default), 1);
        let mut pinned = DecisionPolicy::engine_default();
        pinned.knobs.insert(
            KnobName::new("round_verdict.flip_fraction"),
            KnobValue::DecimalMicros(750_000),
        );
        pinned
            .knobs
            .insert(KnobName::new("round_verdict.rounds"), KnobValue::Integer(4));
        assert_eq!(flip_fraction_micros(&pinned), 750_000);
        assert_eq!(rounds(&pinned), 4);
        for spec in KNOBS {
            assert!(spec.admits(&spec.default));
        }
    }
}
