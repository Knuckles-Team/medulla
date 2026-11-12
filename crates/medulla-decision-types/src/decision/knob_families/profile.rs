//! The `profile.*` knobs (MEDULLA-DECISION-ENGINE-R179.8).

use crate::decision::knobs::{KnobSafety, KnobSpec, KnobValue};

/// Every `profile.*` knob, each `Tunable` unless stated.
pub const KNOBS: &[KnobSpec] = &[
    KnobSpec {
        name: "profile.instructions_max_chars",
        unit: "characters",
        default: KnobValue::Integer(8000),
        min: KnobValue::Integer(200),
        max: KnobValue::Integer(64000),
        safety: KnobSafety::Tunable,
    },
    KnobSpec {
        name: "profile.document_set_max_members",
        unit: "IRIs",
        default: KnobValue::Integer(5000),
        min: KnobValue::Integer(10),
        max: KnobValue::Integer(100000),
        safety: KnobSafety::Tunable,
    },
];
