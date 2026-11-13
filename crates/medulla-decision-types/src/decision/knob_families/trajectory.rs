//! The `trajectory.*`, `breadth.*` and `pairwise.*` knobs of the TaskPlan
//! trajectory decisions (MEDULLA-DECISION-ENGINE-R140.100 to R140.103).
//!
//! Every limit the trajectory decisions apply is a knob read through the
//! pinned policy; a value the policy pins outside the declared range falls
//! back to the nearest bound, so a decision never runs on an undeclared limit.

use crate::decision::knobs::{KnobSafety, KnobSpec, KnobValue};
use crate::decision::policy::DecisionPolicy;

const fn integer(
    name: &'static str,
    unit: &'static str,
    default: i64,
    min: i64,
    max: i64,
) -> KnobSpec {
    KnobSpec {
        name,
        unit,
        default: KnobValue::Integer(default),
        min: KnobValue::Integer(min),
        max: KnobValue::Integer(max),
        safety: KnobSafety::Tunable,
    }
}

/// Score at which a branch is complex, in micro-score (R140.100).
pub const COMPLEXITY_THRESHOLD: KnobSpec = integer(
    "trajectory.complexity_threshold",
    "micro-score",
    1_000_000,
    0,
    1_000_000_000_000,
);
/// Most trajectories a complex branch may run (R140.100).
pub const MAX_K: KnobSpec = integer("trajectory.max_k", "trajectories", 8, 2, 64);
/// Oldest a complexity feature observation may be (R140.100).
pub const FEATURE_MAX_AGE: KnobSpec =
    integer("trajectory.feature_max_age", "seconds", 3600, 1, 604_800);
/// Breadth of a branch with no history (R140.102).
pub const BREADTH_INITIAL: KnobSpec = integer("breadth.initial", "branches", 2, 1, 16);
/// How many latest attempts move the breadth (R140.102).
pub const BREADTH_WINDOW: KnobSpec = integer("breadth.window", "attempts", 3, 1, 20);
/// Largest breadth (R140.102).
pub const BREADTH_MAX: KnobSpec = integer("breadth.max", "branches", 8, 1, 64);
/// Breadth change of a successful attempt (R140.102).
pub const BREADTH_DELTA_SUCCESS: KnobSpec = integer("breadth.delta_success", "branches", -1, -4, 0);
/// Breadth change of a partial attempt (R140.102).
pub const BREADTH_DELTA_PARTIAL: KnobSpec = integer("breadth.delta_partial", "branches", 0, -4, 4);
/// Breadth change of a failed attempt (R140.102).
pub const BREADTH_DELTA_FAILURE: KnobSpec = integer("breadth.delta_failure", "branches", 1, 0, 4);
/// How many candidates a pairwise ranking keeps (R140.103).
pub const PAIRWISE_TOP_K: KnobSpec = integer("pairwise.top_k", "candidates", 3, 1, 64);
/// Revision rounds allowed for the candidates not kept (R140.103).
pub const PAIRWISE_MAX_REVISION_ROUNDS: KnobSpec =
    integer("pairwise.max_revision_rounds", "rounds", 2, 0, 8);

/// Every trajectory-decision knob, each `Tunable`.
pub const KNOBS: &[KnobSpec] = &[
    COMPLEXITY_THRESHOLD,
    MAX_K,
    FEATURE_MAX_AGE,
    BREADTH_INITIAL,
    BREADTH_WINDOW,
    BREADTH_MAX,
    BREADTH_DELTA_SUCCESS,
    BREADTH_DELTA_PARTIAL,
    BREADTH_DELTA_FAILURE,
    PAIRWISE_TOP_K,
    PAIRWISE_MAX_REVISION_ROUNDS,
];

/// The integer the policy pins for `spec`, or its default, held inside the
/// declared range.
pub fn read(policy: &DecisionPolicy, spec: &KnobSpec) -> i64 {
    match (policy.knob(spec), spec.min, spec.max) {
        (KnobValue::Integer(v), KnobValue::Integer(lo), KnobValue::Integer(hi)) => v.clamp(lo, hi),
        (_, _, KnobValue::Integer(_)) => match spec.default {
            KnobValue::Integer(d) => d,
            _ => 0,
        },
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decision::knobs::KnobName;

    // spec: MEDULLA-DECISION-ENGINE-R140.100, MEDULLA-DECISION-ENGINE-R140.102, MEDULLA-DECISION-ENGINE-R140.103
    #[test]
    fn defaults_are_inside_their_ranges_and_unique() {
        let mut names: Vec<&str> = KNOBS.iter().map(|spec| spec.name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), KNOBS.len());
        for spec in KNOBS {
            assert!(spec.admits(&spec.default), "{}", spec.name);
            assert_eq!(spec.safety, KnobSafety::Tunable);
        }
        assert_eq!(MAX_K.default, KnobValue::Integer(8));
        assert_eq!(
            BREADTH_DELTA_SUCCESS.default,
            KnobValue::Integer(-1),
            "{}",
            KnobName::new(BREADTH_DELTA_SUCCESS.name).as_str()
        );
    }
}
