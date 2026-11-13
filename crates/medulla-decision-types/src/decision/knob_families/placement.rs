//! The `placement.*` policy knobs (MEDULLA-PLACEMENT-R001.7): the caps a
//! placement request may only tighten. The pinned policy supplies them; a
//! knob left unpinned is no cap at all.

use crate::decision::knobs::{KnobSafety, KnobSpec, KnobValue};
use crate::decision::policy::DecisionPolicy;
use crate::decision::statistical::placement::{PlacementPolicy, PlacementTarget};

/// No cost cap: the knob's own upper bound.
pub const NO_COST_CAP: i64 = i64::MAX;

/// Every `placement.*` knob, each `Tunable`. The four `forbid_*` knobs are
/// 0 (allowed) or 1 (forbidden), one per registered target in
/// `PlacementTarget::ALL` order.
pub const KNOBS: &[KnobSpec] = &[
    KnobSpec {
        name: "placement.max_relative_cost",
        unit: "cost",
        default: KnobValue::Integer(NO_COST_CAP),
        min: KnobValue::Integer(0),
        max: KnobValue::Integer(NO_COST_CAP),
        safety: KnobSafety::Tunable,
    },
    KnobSpec {
        name: "placement.forbid_in_process",
        unit: "flag",
        default: KnobValue::Integer(0),
        min: KnobValue::Integer(0),
        max: KnobValue::Integer(1),
        safety: KnobSafety::Tunable,
    },
    KnobSpec {
        name: "placement.forbid_compose",
        unit: "flag",
        default: KnobValue::Integer(0),
        min: KnobValue::Integer(0),
        max: KnobValue::Integer(1),
        safety: KnobSafety::Tunable,
    },
    KnobSpec {
        name: "placement.forbid_helm",
        unit: "flag",
        default: KnobValue::Integer(0),
        min: KnobValue::Integer(0),
        max: KnobValue::Integer(1),
        safety: KnobSafety::Tunable,
    },
    KnobSpec {
        name: "placement.forbid_swarm",
        unit: "flag",
        default: KnobValue::Integer(0),
        min: KnobValue::Integer(0),
        max: KnobValue::Integer(1),
        safety: KnobSafety::Tunable,
    },
];

/// The policy caps the pinned policy sets, read through the knobs.
pub fn policy_from_knobs(policy: &DecisionPolicy) -> PlacementPolicy {
    let max_relative_cost = match policy.knob(&KNOBS[0]) {
        KnobValue::Integer(cap) if cap != NO_COST_CAP => u64::try_from(cap).ok(),
        _ => None,
    };
    let forbidden_targets = PlacementTarget::ALL
        .iter()
        .zip(&KNOBS[1..])
        .filter(|(_, spec)| matches!(policy.knob(spec), KnobValue::Integer(1)))
        .map(|(target, _)| *target)
        .collect();
    PlacementPolicy {
        max_relative_cost,
        forbidden_targets,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decision::knobs::KnobName;

    // spec: MEDULLA-PLACEMENT-R001.7
    #[test]
    fn defaults_cap_nothing_and_a_pinned_knob_tightens() {
        for spec in KNOBS {
            assert!(spec.admits(&spec.default), "{}", spec.name);
        }
        let open = policy_from_knobs(&DecisionPolicy::engine_default());
        assert_eq!(open, PlacementPolicy::default());
        let mut pinned = DecisionPolicy::engine_default();
        pinned.knobs.insert(
            KnobName::new("placement.max_relative_cost"),
            KnobValue::Integer(5),
        );
        pinned.knobs.insert(
            KnobName::new("placement.forbid_swarm"),
            KnobValue::Integer(1),
        );
        let caps = policy_from_knobs(&pinned);
        assert_eq!(caps.max_relative_cost, Some(5));
        assert_eq!(caps.forbidden_targets, vec![PlacementTarget::Swarm]);
    }
}
