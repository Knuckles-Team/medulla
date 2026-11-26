//! The `memory.*` knobs (MEDULLA-DECISION-ENGINE-R179.8, R154.1).

use crate::decision::knobs::{KnobSafety, KnobSpec, KnobValue};

const MINUTE_MS: u64 = 60_000;
const HOUR_MS: u64 = 60 * MINUTE_MS;
const DAY_MS: u64 = 24 * HOUR_MS;

/// The most importance a reinforced memory can hold
/// (MEDULLA-DECISION-ENGINE-R651.8).
pub const IMPORTANCE_CAP: KnobSpec = KnobSpec {
    name: "memory.importance_cap",
    unit: "importance",
    default: KnobValue::DecimalMicros(10_000_000),
    min: KnobValue::DecimalMicros(1_000_000),
    max: KnobValue::DecimalMicros(100_000_000),
    safety: KnobSafety::Tunable,
};

/// The fewest episodes a `Consolidate` request naming a `pattern_key` may send
/// (MEDULLA-DECISION-ENGINE-R651.7).
pub const PROMOTE_MIN_SUPPORT: KnobSpec = KnobSpec {
    name: "memory.promote_min_support",
    unit: "episodes",
    default: KnobValue::Integer(3),
    min: KnobValue::Integer(2),
    max: KnobValue::Integer(50),
    safety: KnobSafety::Tunable,
};

/// The most bytes one memory record may store: the msgpack property map written as its
/// node (MEDULLA-DECISION-ENGINE-R651.4).
pub const ENTRY_MAX_BYTES: KnobSpec = KnobSpec {
    name: "memory.entry_max_bytes",
    unit: "bytes",
    default: KnobValue::Integer(65_536),
    min: KnobValue::Integer(1_024),
    max: KnobValue::Integer(1_048_576),
    safety: KnobSafety::Fixed,
};

/// The most bytes a tenant's memory records may store in total: the sum of the stored node
/// bytes of its `memory:<tenant>:` nodes (MEDULLA-DECISION-ENGINE-R154.1).
pub const TENANT_MAX_BYTES: KnobSpec = KnobSpec {
    name: "memory.tenant_max_bytes",
    unit: "bytes",
    default: KnobValue::Integer(1_073_741_824),
    min: KnobValue::Integer(1_048_576),
    max: KnobValue::Integer(1_099_511_627_776),
    safety: KnobSafety::Fixed,
};

/// The most bytes one saved search state may hold
/// (MEDULLA-DECISION-ENGINE-R149.1, R149.4).
pub const SEARCH_STATE_MAX_BYTES: KnobSpec = KnobSpec {
    name: "memory.search_state_max_bytes",
    unit: "bytes",
    default: KnobValue::Integer(1_048_576),
    min: KnobValue::Integer(65_536),
    max: KnobValue::Integer(1_048_576),
    safety: KnobSafety::Tunable,
};

/// The default decay half-life of the working tier.
pub const HALF_LIFE_WORKING: KnobSpec = KnobSpec {
    name: "memory.half_life_working",
    unit: "ms",
    default: KnobValue::DurationMs(HOUR_MS),
    min: KnobValue::DurationMs(MINUTE_MS),
    max: KnobValue::DurationMs(24 * HOUR_MS),
    safety: KnobSafety::Tunable,
};

/// The default decay half-life of the episodic tier.
pub const HALF_LIFE_EPISODIC: KnobSpec = KnobSpec {
    name: "memory.half_life_episodic",
    unit: "ms",
    default: KnobValue::DurationMs(7 * DAY_MS),
    min: KnobValue::DurationMs(DAY_MS),
    max: KnobValue::DurationMs(90 * DAY_MS),
    safety: KnobSafety::Tunable,
};

/// The default decay half-life of the procedural tier.
pub const HALF_LIFE_PROCEDURAL: KnobSpec = KnobSpec {
    name: "memory.half_life_procedural",
    unit: "ms",
    default: KnobValue::DurationMs(90 * DAY_MS),
    min: KnobValue::DurationMs(7 * DAY_MS),
    max: KnobValue::DurationMs(730 * DAY_MS),
    safety: KnobSafety::Tunable,
};

/// The default decay half-life of the semantic tier.
pub const HALF_LIFE_SEMANTIC: KnobSpec = KnobSpec {
    name: "memory.half_life_semantic",
    unit: "ms",
    default: KnobValue::DurationMs(365 * DAY_MS),
    min: KnobValue::DurationMs(30 * DAY_MS),
    max: KnobValue::DurationMs(1825 * DAY_MS),
    safety: KnobSafety::Tunable,
};

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
    IMPORTANCE_CAP,
    PROMOTE_MIN_SUPPORT,
    ENTRY_MAX_BYTES,
    TENANT_MAX_BYTES,
    SEARCH_STATE_MAX_BYTES,
    HALF_LIFE_WORKING,
    HALF_LIFE_EPISODIC,
    HALF_LIFE_PROCEDURAL,
    HALF_LIFE_SEMANTIC,
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
