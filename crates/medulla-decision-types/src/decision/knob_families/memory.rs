//! The `memory.*` knobs (MEDULLA-DECISION-ENGINE-R179.8, R154.1).

use crate::decision::knobs::{KnobSafety, KnobSpec, KnobValue};

/// Every `memory.*` knob, each `Tunable` unless stated.
pub const KNOBS: &[KnobSpec] = &[
    KnobSpec {
        name: "memory.derive_batch_turns",
        unit: "turns",
        default: KnobValue::Integer(8),
        min: KnobValue::Integer(1),
        max: KnobValue::Integer(64),
        safety: KnobSafety::Tunable,
    },
    KnobSpec {
        name: "memory.derive_debounce",
        unit: "ms",
        default: KnobValue::DurationMs(30000),
        min: KnobValue::DurationMs(1000),
        max: KnobValue::DurationMs(600000),
        safety: KnobSafety::Tunable,
    },
    KnobSpec {
        name: "memory.derive_max_pending",
        unit: "tasks",
        default: KnobValue::Integer(10000),
        min: KnobValue::Integer(100),
        max: KnobValue::Integer(1000000),
        safety: KnobSafety::Tunable,
    },
    KnobSpec {
        name: "memory.derive_max_conclusions",
        unit: "claims",
        default: KnobValue::Integer(8),
        min: KnobValue::Integer(1),
        max: KnobValue::Integer(32),
        safety: KnobSafety::Tunable,
    },
    KnobSpec {
        name: "memory.entry_max_bytes",
        unit: "bytes",
        default: KnobValue::Integer(65536),
        min: KnobValue::Integer(1024),
        max: KnobValue::Integer(1048576),
        safety: KnobSafety::Fixed,
    },
    KnobSpec {
        name: "memory.tenant_max_bytes",
        unit: "bytes",
        default: KnobValue::Integer(1073741824),
        min: KnobValue::Integer(1048576),
        max: KnobValue::Integer(1099511627776),
        safety: KnobSafety::Fixed,
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-DECISION-ENGINE-R154.1
    #[test]
    fn memory_knobs_cover_every_policy_field() {
        // Every retention and admission parameter is a knob of the `memory`
        // family; the operator has no per-corpus override.
        for required in ["memory.entry_max_bytes", "memory.tenant_max_bytes"] {
            assert!(
                KNOBS.iter().any(|spec| spec.name == required),
                "{required} has no KnobSpec"
            );
        }
        for spec in KNOBS {
            assert!(spec.name.starts_with("memory."), "{}", spec.name);
            assert!(
                spec.admits(&spec.default),
                "{} default is outside its bounds",
                spec.name
            );
            assert!(
                spec.min.compare(&spec.max) == Some(std::cmp::Ordering::Less),
                "{} bounds are not ordered",
                spec.name
            );
        }
        let names: std::collections::BTreeSet<_> = KNOBS.iter().map(|spec| spec.name).collect();
        assert_eq!(names.len(), KNOBS.len(), "duplicate memory knob");
        for fixed in ["memory.entry_max_bytes", "memory.tenant_max_bytes"] {
            let spec = KNOBS.iter().find(|spec| spec.name == fixed).unwrap();
            assert_eq!(spec.safety, KnobSafety::Fixed, "{fixed}");
        }
    }
}
