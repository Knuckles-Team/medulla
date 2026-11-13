//! The `red_team.*` knobs (MEDULLA-DECISION-ENGINE-R138.3). They fill the
//! probe-verdict defaults, pinned through the decision policy.

use crate::decision::knobs::{KnobSafety, KnobSpec, KnobValue};
use crate::decision::policy::DecisionPolicy;

/// The `red_team.*` knobs, each a `Fixed` flag.
pub const KNOBS: &[KnobSpec] = &[
    KnobSpec {
        name: "red_team.category_match",
        unit: "flag",
        default: KnobValue::Integer(1),
        min: KnobValue::Integer(0),
        max: KnobValue::Integer(1),
        safety: KnobSafety::Fixed,
    },
    KnobSpec {
        name: "red_team.any_scorer_compromises",
        unit: "flag",
        default: KnobValue::Integer(1),
        min: KnobValue::Integer(0),
        max: KnobValue::Integer(1),
        safety: KnobSafety::Fixed,
    },
];

fn flag(policy: &DecisionPolicy, spec: &KnobSpec) -> bool {
    match policy.knob(spec) {
        KnobValue::Integer(v) => v != 0,
        _ => true,
    }
}

/// Whether only scorers of the attack's category are selected.
pub fn category_match(policy: &DecisionPolicy) -> bool {
    flag(policy, &KNOBS[0])
}

/// Whether one passing scorer is enough to call a probe compromised.
pub fn any_scorer_compromises(policy: &DecisionPolicy) -> bool {
    flag(policy, &KNOBS[1])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decision::knobs::KnobName;

    // spec: MEDULLA-DECISION-ENGINE-R138.3
    #[test]
    fn knobs_read_defaults_and_pinned_values() {
        let default = DecisionPolicy::engine_default();
        assert!(category_match(&default));
        assert!(any_scorer_compromises(&default));
        let mut pinned = DecisionPolicy::engine_default();
        pinned.knobs.insert(
            KnobName::new("red_team.category_match"),
            KnobValue::Integer(0),
        );
        assert!(!category_match(&pinned));
        for spec in KNOBS {
            assert!(spec.admits(&spec.default));
        }
    }
}
