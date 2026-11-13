//! The `prompt_candidate.*` knobs (MEDULLA-DECISION-ENGINE-R814.2).

use crate::decision::knobs::{KnobSafety, KnobSpec, KnobValue};

/// Every `prompt_candidate.*` knob.
pub const KNOBS: &[KnobSpec] = &[KnobSpec {
    name: "prompt_candidate.min_match_ratio",
    unit: "fraction",
    default: KnobValue::DecimalMicros(1_000_000),
    min: KnobValue::DecimalMicros(500_000),
    max: KnobValue::DecimalMicros(1_000_000),
    safety: KnobSafety::Tunable,
}];
