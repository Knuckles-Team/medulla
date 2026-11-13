//! The `verifier.*` knobs (MEDULLA-DECISION-ENGINE-R138.100.1): how many attack
//! cases a verifier receipt must hold before generated code can be admitted.

use crate::decision::knobs::{KnobSafety, KnobSpec, KnobValue};
use crate::decision::policy::DecisionPolicy;

/// The `verifier.*` knobs. The minimum is `Fixed`: a revision may not move it.
pub const KNOBS: &[KnobSpec] = &[KnobSpec {
    name: "verifier.min_cases",
    unit: "count",
    default: KnobValue::Integer(1),
    min: KnobValue::Integer(1),
    max: KnobValue::Integer(64),
    safety: KnobSafety::Fixed,
}];

/// The fewest cases a receipt of `policy` carries.
pub fn min_cases(policy: &DecisionPolicy) -> usize {
    match policy.knob(&KNOBS[0]) {
        KnobValue::Integer(v) => usize::try_from(v).unwrap_or(1),
        _ => 1,
    }
}
