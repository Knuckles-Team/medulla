//! The `topology.*` cap knobs (MEDULLA-DECISION-ENGINE-R046.750). The upper
//! bounds are the engine constants a plan may never exceed; a knob-set
//! revision may only tighten them inside `[1, constant]`.

use medulla_agent_types::agent_topology::{
    TopologyCaps, MAX_SLOT_ROUNDS, MAX_SLOT_WIDTH, MAX_TOPOLOGY_DEPTH,
};

use crate::decision::knobs::{KnobSafety, KnobSpec, KnobValue};
use crate::decision::policy::DecisionPolicy;

/// Every `topology.*` knob, each `Tunable`.
pub const KNOBS: &[KnobSpec] = &[
    KnobSpec {
        name: "topology.max_width",
        unit: "count",
        default: KnobValue::Integer(16),
        min: KnobValue::Integer(1),
        max: KnobValue::Integer(16),
        safety: KnobSafety::Tunable,
    },
    KnobSpec {
        name: "topology.max_depth",
        unit: "count",
        default: KnobValue::Integer(8),
        min: KnobValue::Integer(1),
        max: KnobValue::Integer(8),
        safety: KnobSafety::Tunable,
    },
    KnobSpec {
        name: "topology.max_rounds",
        unit: "count",
        default: KnobValue::Integer(8),
        min: KnobValue::Integer(1),
        max: KnobValue::Integer(8),
        safety: KnobSafety::Tunable,
    },
];

/// The caps the pinned policy sets: each knob, or its default, as a cap.
/// Tokens are not a knob; the request and the topology policy name them.
pub fn caps_from_policy(policy: &DecisionPolicy) -> TopologyCaps {
    let read = |spec: &KnobSpec, bound: u8| match policy.knob(spec) {
        KnobValue::Integer(value) => u8::try_from(value).unwrap_or(bound).clamp(1, bound),
        _ => bound,
    };
    TopologyCaps {
        max_width: read(&KNOBS[0], MAX_SLOT_WIDTH),
        max_depth: read(&KNOBS[1], MAX_TOPOLOGY_DEPTH),
        max_rounds: read(&KNOBS[2], MAX_SLOT_ROUNDS),
        max_tokens: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decision::knobs::KnobName;

    // spec: MEDULLA-DECISION-ENGINE-R046.750
    #[test]
    fn defaults_are_the_engine_bounds_and_a_pinned_knob_tightens() {
        let policy = DecisionPolicy::engine_default();
        let caps = caps_from_policy(&policy);
        assert_eq!(
            (caps.max_width, caps.max_depth, caps.max_rounds),
            (MAX_SLOT_WIDTH, MAX_TOPOLOGY_DEPTH, MAX_SLOT_ROUNDS)
        );
        for spec in KNOBS {
            assert!(spec.admits(&spec.default));
            assert_eq!(spec.safety, KnobSafety::Tunable);
        }
        let mut pinned = policy;
        pinned
            .knobs
            .insert(KnobName::new("topology.max_width"), KnobValue::Integer(2));
        assert_eq!(caps_from_policy(&pinned).max_width, 2);
    }
}
