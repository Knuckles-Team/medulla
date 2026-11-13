//! The `schema_mapping.*` score weights (MEDULLA-DECISION-ENGINE-R033.2.1):
//! how much a column's name, datatype and sample values each count toward its
//! score against an ontology property. Each is a fraction in `[0, 1]`, stored as
//! micros.

use crate::decision::knobs::{KnobSafety, KnobSpec, KnobValue};
use crate::decision::policy::DecisionPolicy;

const fn weight(name: &'static str, default_micros: i64) -> KnobSpec {
    KnobSpec {
        name,
        unit: "fraction",
        default: KnobValue::DecimalMicros(default_micros),
        min: KnobValue::DecimalMicros(0),
        max: KnobValue::DecimalMicros(1_000_000),
        safety: KnobSafety::Tunable,
    }
}

/// Weight of the token Jaccard of the column and property names.
pub const W_NAME: KnobSpec = weight("schema_mapping.w_name", 500_000);
/// Weight of the datatype compatibility.
pub const W_TYPE: KnobSpec = weight("schema_mapping.w_type", 300_000);
/// Weight of the fraction of sample values that validate.
pub const W_SAMPLE: KnobSpec = weight("schema_mapping.w_sample", 200_000);

/// Every `schema_mapping.*` knob, each `Tunable`.
pub const KNOBS: &[KnobSpec] = &[W_NAME, W_TYPE, W_SAMPLE];

/// The weights the pinned policy sets, in micros: each knob, or its default.
pub fn weights_micros(policy: &DecisionPolicy) -> [i64; 3] {
    let read = |spec: &KnobSpec| match policy.knob(spec) {
        KnobValue::DecimalMicros(micros) => micros.clamp(0, 1_000_000),
        _ => match spec.default {
            KnobValue::DecimalMicros(micros) => micros,
            _ => 0,
        },
    };
    [read(&W_NAME), read(&W_TYPE), read(&W_SAMPLE)]
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-DECISION-ENGINE-R033.2.1
    #[test]
    fn the_weights_are_declared_fractions_with_the_stated_defaults() {
        assert_eq!(
            KNOBS.iter().map(|k| k.name).collect::<Vec<_>>(),
            vec![
                "schema_mapping.w_name",
                "schema_mapping.w_type",
                "schema_mapping.w_sample"
            ]
        );
        for spec in KNOBS {
            assert!(spec.admits(&spec.default));
            assert!(!spec.admits(&KnobValue::DecimalMicros(1_000_001)));
            assert!(!spec.admits(&KnobValue::DecimalMicros(-1)));
        }
        let policy = DecisionPolicy::engine_default();
        assert_eq!(weights_micros(&policy), [500_000, 300_000, 200_000]);
    }
}
