//! The `orchestration_limits.*` knobs of the harness session index
//! (MEDULLA-ORCH-VOCAB-R009.101.1, R009.101.2).

use crate::decision::knobs::{KnobSafety, KnobSpec, KnobValue};

/// Every `orchestration_limits.*` session knob, each `Tunable`.
pub const KNOBS: &[KnobSpec] = &[
    KnobSpec {
        name: "orchestration_limits.session_batch_max",
        unit: "sessions",
        default: KnobValue::Integer(500),
        min: KnobValue::Integer(1),
        max: KnobValue::Integer(5000),
        safety: KnobSafety::Tunable,
    },
    KnobSpec {
        name: "orchestration_limits.page_size_max",
        unit: "rows",
        default: KnobValue::Integer(100),
        min: KnobValue::Integer(1),
        max: KnobValue::Integer(1000),
        safety: KnobSafety::Tunable,
    },
];
