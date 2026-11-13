//! Branch outcome and breadth-adaptation records (MEDULLA-DECISION-ENGINE-R164.1).

use serde::{Deserialize, Serialize};

use medulla_decision_types::decision::knobs::{KnobSafety, KnobSpec, KnobValue};
use medulla_types::contract::OpaqueId;

/// The `breadth.*` knobs: `min`, `max`, `raise_at`, `narrow_at`, `min_attempts`.
/// Rates are fixed-point decimals (the value times 1,000,000).
pub const KNOBS: &[KnobSpec] = &[
    KnobSpec {
        name: "breadth.min",
        unit: "branches",
        default: KnobValue::Integer(1),
        min: KnobValue::Integer(1),
        max: KnobValue::Integer(8),
        safety: KnobSafety::Tunable,
    },
    KnobSpec {
        name: "breadth.max",
        unit: "branches",
        default: KnobValue::Integer(8),
        min: KnobValue::Integer(1),
        max: KnobValue::Integer(64),
        safety: KnobSafety::Tunable,
    },
    KnobSpec {
        name: "breadth.raise_at",
        unit: "rate",
        default: KnobValue::DecimalMicros(700_000),
        min: KnobValue::DecimalMicros(500_000),
        max: KnobValue::DecimalMicros(1_000_000),
        safety: KnobSafety::Tunable,
    },
    KnobSpec {
        name: "breadth.narrow_at",
        unit: "rate",
        default: KnobValue::DecimalMicros(300_000),
        min: KnobValue::DecimalMicros(0),
        max: KnobValue::DecimalMicros(500_000),
        safety: KnobSafety::Tunable,
    },
    KnobSpec {
        name: "breadth.min_attempts",
        unit: "attempts",
        default: KnobValue::Integer(5),
        min: KnobValue::Integer(1),
        max: KnobValue::Integer(1000),
        safety: KnobSafety::Tunable,
    },
];

/// One attempt at one branch: whether it succeeded and what it cost.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BranchOutcomeObservation {
    pub branch_id: OpaqueId,
    pub succeeded: bool,
    pub cost: u64,
}

/// The breadth bounds and the success rates that raise or narrow it. Rates are
/// micro-units in `[0, 1000000]`; the values come from the `breadth.*` knobs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BreadthPolicy {
    pub min: u32,
    pub max: u32,
    pub raise_at: u32,
    pub narrow_at: u32,
    pub min_attempts: u32,
}

impl BreadthPolicy {
    /// `min` is at most `max` and `narrow_at` is below `raise_at`.
    pub fn checked(self) -> Result<Self, String> {
        if self.min == 0 || self.min > self.max {
            return Err("breadth min must be at least 1 and at most max".into());
        }
        if self.raise_at > 1_000_000 || self.narrow_at >= self.raise_at {
            return Err("narrow_at must be below raise_at, both at most 1000000".into());
        }
        Ok(self)
    }
}

/// Aggregate outcomes observed for one branch.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BranchOutcome {
    pub branch_id: OpaqueId,
    pub attempt_count: u64,
    pub success_count: u64,
    pub average_cost: u64,
}

/// Prior-branch outcome evidence and whether it was applied to adaptation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutcomeEvidence {
    pub prior_branch_id: OpaqueId,
    pub outcome: BranchOutcome,
    pub adaptation_applied: bool,
}

/// A breadth change linked to the outcome that triggered it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BreadthAdaptation {
    pub branch_id: OpaqueId,
    pub prior_breadth: u32,
    pub new_breadth: u32,
    pub outcome_triggering: OutcomeEvidence,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(value: &str) -> OpaqueId {
        OpaqueId::new(value).unwrap()
    }

    // spec: MEDULLA-DECISION-ENGINE-R164.1
    #[test]
    fn outcome_types_round_trip() {
        let outcome = BranchOutcome {
            branch_id: id("branch:current"),
            attempt_count: 12,
            success_count: 9,
            average_cost: 420,
        };
        let evidence = OutcomeEvidence {
            prior_branch_id: id("branch:prior"),
            outcome: outcome.clone(),
            adaptation_applied: true,
        };
        let adaptation = BreadthAdaptation {
            branch_id: outcome.branch_id.clone(),
            prior_breadth: 4,
            new_breadth: 6,
            outcome_triggering: evidence.clone(),
        };

        let outcome_bytes = rmp_serde::to_vec_named(&outcome).unwrap();
        assert_eq!(
            rmp_serde::from_slice::<BranchOutcome>(&outcome_bytes).unwrap(),
            outcome
        );
        let evidence_bytes = rmp_serde::to_vec_named(&evidence).unwrap();
        assert_eq!(
            rmp_serde::from_slice::<OutcomeEvidence>(&evidence_bytes).unwrap(),
            evidence
        );
        let adaptation_bytes = rmp_serde::to_vec_named(&adaptation).unwrap();
        assert_eq!(
            rmp_serde::from_slice::<BreadthAdaptation>(&adaptation_bytes).unwrap(),
            adaptation
        );
    }

    // spec: MEDULLA-DECISION-ENGINE-R164.1
    #[test]
    fn policy_and_observation_round_trip() {
        let observation = BranchOutcomeObservation {
            branch_id: id("branch:a"),
            succeeded: true,
            cost: 12,
        };
        let bytes = rmp_serde::to_vec_named(&observation).unwrap();
        assert_eq!(
            rmp_serde::from_slice::<BranchOutcomeObservation>(&bytes).unwrap(),
            observation
        );
        let policy = BreadthPolicy {
            min: 1,
            max: 8,
            raise_at: 700_000,
            narrow_at: 300_000,
            min_attempts: 5,
        };
        let bytes = rmp_serde::to_vec_named(&policy).unwrap();
        assert_eq!(
            rmp_serde::from_slice::<BreadthPolicy>(&bytes).unwrap(),
            policy
        );
        assert!(policy.clone().checked().is_ok());
        assert!(BreadthPolicy {
            min: 9,
            ..policy.clone()
        }
        .checked()
        .is_err());
        assert!(BreadthPolicy {
            narrow_at: 700_000,
            ..policy.clone()
        }
        .checked()
        .is_err());
        for spec in KNOBS {
            assert!(spec.admits(&spec.default), "{}", spec.name);
        }
    }
}
