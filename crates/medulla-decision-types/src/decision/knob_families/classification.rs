//! The `classification.*` knobs (MEDULLA-TYPED-PACKS-R652.2).

use crate::decision::knobs::{KnobSafety, KnobSpec, KnobValue};

/// Index of `classification.propagation_depth_max` in [`KNOBS`].
pub const PROPAGATION_DEPTH_MAX: usize = 0;

/// Every `classification.*` knob.
pub const KNOBS: &[KnobSpec] = &[KnobSpec {
    name: "classification.propagation_depth_max",
    unit: "hops",
    default: KnobValue::Integer(64),
    min: KnobValue::Integer(8),
    max: KnobValue::Integer(1024),
    safety: KnobSafety::Tunable,
}];
