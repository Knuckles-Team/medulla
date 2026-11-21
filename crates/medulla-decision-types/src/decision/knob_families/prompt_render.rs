//! The `prompt.*` knobs (MEDULLA-TYPED-PACKS-R002.9).

use crate::decision::knobs::{KnobSafety, KnobSpec, KnobValue};

/// Index of `prompt.max_render_bytes` in [`KNOBS`].
pub const MAX_RENDER_BYTES: usize = 0;

/// Every `prompt.*` knob. The limit bounds the bytes one served render may
/// return, so a knob-set revision may tune it inside the closed range.
pub const KNOBS: &[KnobSpec] = &[KnobSpec {
    name: "prompt.max_render_bytes",
    unit: "bytes",
    default: KnobValue::Integer(1_048_576),
    min: KnobValue::Integer(1_024),
    max: KnobValue::Integer(16_777_216),
    safety: KnobSafety::Tunable,
}];
