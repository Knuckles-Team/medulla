//! The `placement_plan` question family (MEDULLA-PLACEMENT-R700.3).
//!
//! Services onto hosts within their CPU, memory and storage budgets. The family
//! concludes through `stat_placement_plan`: the bounded 0-1 solve over the host
//! budgets the server recorded in the question, under the pinned policy's
//! `placement.node_budget` knob. Evaluate-only.

use medulla_decision_types::decision::statistical::outcome::FamilyAnswer;
use medulla_decision_types::decision::statistical::DecideRequest;
use medulla_graph_core::graph::GraphView;
use {
    medulla_decision_statistical_types::decision::record::premise::PremiseRef,
    medulla_decision_statistical_types::decision::DecisionPolicy,
};

use crate::decide::stat_executor::{Executed, ExecutionContext, FamilyExecuted};
use crate::decide::stat_placement_plan::{placement_plan_outcome, premises, question_of};

use super::DecisionFamily;

/// The family of the `placement_plan` question kind.
pub struct PlacementPlanFamily;

impl DecisionFamily for PlacementPlanFamily {
    fn constraints(&self, request: &DecideRequest) -> Result<(), String> {
        question_of(request).map(|_| ())
    }

    fn conclude(
        &self,
        _ctx: &ExecutionContext,
        request: &DecideRequest,
        policy: &DecisionPolicy,
        _graph_view: Option<&GraphView>,
    ) -> Option<Result<Executed, String>> {
        Some(
            placement_plan_outcome(request, policy).map(|(outcome, plan)| {
                Executed::from_family(
                    outcome,
                    plan.map(|plan| FamilyExecuted {
                        answer: Some(FamilyAnswer::PlacementPlan(Box::new(plan))),
                        premises: Vec::new(),
                        resolution: None,
                        evidence: None,
                    }),
                )
            }),
        )
    }

    fn premises(
        &self,
        request: &DecideRequest,
        _executed: &Executed,
        _policy_digest: &str,
    ) -> Vec<PremiseRef> {
        premises(request)
    }
}
