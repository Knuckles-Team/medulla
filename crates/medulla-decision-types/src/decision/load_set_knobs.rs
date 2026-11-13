//! The `load_set.*` knobs (MEDULLA-DECISION-ENGINE-R176.2), registered through
//! [`super::knob_families::REGISTRY`] and read through the policy.

use super::knobs::{KnobSafety, KnobSpec, KnobValue};
use super::policy::DecisionPolicy;

/// Designations asked per required capability, and the cap on skills.
pub const KNOBS: &[KnobSpec] = &[
    KnobSpec {
        name: "load_set.per_capability_k",
        unit: "components",
        default: KnobValue::Integer(8),
        min: KnobValue::Integer(1),
        max: KnobValue::Integer(32),
        safety: KnobSafety::Tunable,
    },
    KnobSpec {
        name: "load_set.max_skills",
        unit: "skills",
        default: KnobValue::Integer(16),
        min: KnobValue::Integer(0),
        max: KnobValue::Integer(64),
        safety: KnobSafety::Tunable,
    },
];

/// The resolved `load_set.*` knobs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoadSetKnobs {
    pub per_capability_k: usize,
    pub max_skills: usize,
}

impl Default for LoadSetKnobs {
    fn default() -> Self {
        Self::from_policy(&DecisionPolicy::engine_default())
    }
}

impl LoadSetKnobs {
    /// The knobs a policy pins, with the spec default for the rest.
    pub fn from_policy(policy: &DecisionPolicy) -> Self {
        let int = |index: usize| match policy.knob(&KNOBS[index]) {
            KnobValue::Integer(v) => v.max(0) as usize,
            _ => 0,
        };
        Self {
            per_capability_k: int(0),
            max_skills: int(1),
        }
    }
}
