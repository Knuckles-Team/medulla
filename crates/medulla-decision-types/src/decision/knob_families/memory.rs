//! The `memory.*` knobs (MEDULLA-DECISION-ENGINE-R179.8).

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
];
