//! The trajectory decisions a `task_plan` Decide can carry
//! (MEDULLA-DECISION-ENGINE-R140.104).
//!
//! One typed parameter names which of the four pure decisions to take
//! ([`TrajectoryQuery`]); the answer rides on the returned plan as a
//! [`TrajectoryDecision`] that binds the decision digest, the pinned policy
//! digest and every source digest the decision read.

use serde::{Deserialize, Serialize};

use super::branch_breadth::{BreadthDecision, BreadthRequest};
use super::pairwise::{PairwiseRanking, PairwiseRequest};
use {
    medulla_decision_policy_types::decision::task_plan::trajectory_budget::TrajectoryBudget,
    medulla_decision_policy_types::decision::task_plan::trajectory_budget::TrajectoryBudgetRequest,
};
use {
    medulla_decision_policy_types::decision::task_plan::trajectory_projection::ProjectionPlan,
    medulla_decision_policy_types::decision::task_plan::trajectory_projection::ProjectionRequest,
};

/// The parameter name a `task_plan` request carries a trajectory query under.
pub const TRAJECTORY_PARAM: &str = "trajectory";

/// Which trajectory decision a request asks for, with its request body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum TrajectoryQuery {
    /// The complexity-gated K budget (R140.100).
    Budget(TrajectoryBudgetRequest),
    /// The provenance-preserving projection (R140.101).
    Projection(ProjectionRequest),
    /// The outcome-driven breadth (R140.102).
    Breadth(BreadthRequest),
    /// The pairwise Copeland ranking (R140.103).
    Pairwise(PairwiseRequest),
}

/// The decision a query produced.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum TrajectoryAnswer {
    Budget(TrajectoryBudget),
    Projection(ProjectionPlan),
    Breadth(BreadthDecision),
    Pairwise(PairwiseRanking),
}

/// A trajectory decision bound to its digests, carried on the returned plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct TrajectoryDecision {
    pub answer: TrajectoryAnswer,
    /// The digest of the decision itself (the kernel's `decision_digest`).
    pub decision_digest: String,
    /// The digest of the decision policy the knobs were read from.
    pub policy_digest: String,
    /// Every source digest the decision read, sorted and deduplicated:
    /// feature and scorer digests, trace digests, outcome digests or
    /// candidate and evidence digests.
    pub source_digests: Vec<String>,
}

impl TrajectoryQuery {
    /// The source digests the query names, sorted and deduplicated.
    pub fn source_digests(&self) -> Vec<String> {
        let mut digests: Vec<String> = match self {
            Self::Budget(request) => request
                .features
                .iter()
                .map(|f| f.source_digest.clone())
                .chain(request.scorer.iter().map(|s| s.digest.clone()))
                .collect(),
            Self::Projection(request) => request
                .sources
                .iter()
                .map(|s| s.trace_digest.clone())
                .collect(),
            Self::Breadth(request) => match &request.history {
                super::branch_breadth::BranchHistory::Absent => Vec::new(),
                super::branch_breadth::BranchHistory::Present { refs, .. } => {
                    refs.iter().map(|r| r.outcome_digest.clone()).collect()
                }
            },
            Self::Pairwise(request) => request
                .candidates
                .iter()
                .map(|c| c.digest.clone())
                .chain(request.records.iter().map(|r| r.evidence_digest.clone()))
                .collect(),
        };
        digests.sort();
        digests.dedup();
        digests
    }
}

impl TrajectoryAnswer {
    /// The decision's own digest.
    pub fn decision_digest(&self) -> &str {
        match self {
            Self::Budget(d) => &d.decision_digest,
            Self::Projection(d) => &d.decision_digest,
            Self::Breadth(d) => &d.decision_digest,
            Self::Pairwise(d) => &d.decision_digest,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decision::task_plan::branch_breadth::BranchHistory;

    #[test]
    fn query_round_trips_and_names_its_source_digests() {
        let query = TrajectoryQuery::Breadth(BreadthRequest {
            search_id: "s".into(),
            branch_id: "b".into(),
            ordinal: 1,
            plan_digest: "sha256:p".into(),
            history: BranchHistory::Absent,
        });
        let json = serde_json::to_string(&query).unwrap();
        assert!(json.contains("\"decision\":\"breadth\""));
        assert_eq!(
            serde_json::from_str::<TrajectoryQuery>(&json).unwrap(),
            query
        );
        assert!(query.source_digests().is_empty());
    }
}
