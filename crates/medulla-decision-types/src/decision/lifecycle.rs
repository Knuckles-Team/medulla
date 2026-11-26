//! The lifecycle family's result-size knobs (MEDULLA-SDLC-R006.7).
//!
//! The trace's depth ceiling and node budget are Tunable knobs of family
//! `lifecycle`; the served handler reads them through
//! [`super::DecisionPolicy::knob`] and hands the resolved value to the walk
//! as [`medulla_source_types::lifecycle::TraceLimits`].

use super::knobs::{KnobSafety, KnobSpec, KnobValue};

/// The result-size knobs of the lifecycle family: how deep a trace may walk
/// and how many records it may visit (MEDULLA-SDLC-R006.7).
pub const KNOBS: &[KnobSpec] = &[
    KnobSpec {
        name: "lifecycle.trace_max_depth",
        unit: "levels",
        default: KnobValue::Integer(8),
        min: KnobValue::Integer(1),
        max: KnobValue::Integer(16),
        safety: KnobSafety::Tunable,
    },
    KnobSpec {
        name: "lifecycle.trace_node_budget",
        unit: "records",
        default: KnobValue::Integer(10_000),
        min: KnobValue::Integer(100),
        max: KnobValue::Integer(100_000),
        safety: KnobSafety::Tunable,
    },
    // The delivery knobs (MEDULLA-SDLC-R019.4, R019.9, R019.10). Index 2 to 5.
    KnobSpec {
        name: "sdlc.delivery.shadow_unprompted_in_development",
        unit: "flag",
        default: KnobValue::Integer(1),
        min: KnobValue::Integer(1),
        max: KnobValue::Integer(1),
        safety: KnobSafety::Fixed,
    },
    KnobSpec {
        name: "sdlc.delivery.max_automated_review_rounds",
        unit: "rounds",
        default: KnobValue::Integer(3),
        min: KnobValue::Integer(1),
        max: KnobValue::Integer(10),
        safety: KnobSafety::Tunable,
    },
    KnobSpec {
        name: "sdlc.delivery.arm_max_changed_lines",
        unit: "lines",
        default: KnobValue::Integer(400),
        min: KnobValue::Integer(1),
        max: KnobValue::Integer(10_000),
        safety: KnobSafety::Tunable,
    },
    KnobSpec {
        name: "sdlc.delivery.owner_approval_required",
        unit: "flag",
        default: KnobValue::Integer(1),
        min: KnobValue::Integer(1),
        max: KnobValue::Integer(1),
        safety: KnobSafety::Fixed,
    },
];
