//! The `task_plan` question family (MEDULLA-DECISION-ENGINE-R001.101).
//!
//! Bounded explicit DAG planning: the family concludes a typed plan from
//! the typed `task_plan` parameter (evaluate-only; admission belongs to the
//! host).

use medulla_decision_stats::decision::ladder::LadderResult;
use medulla_decision_types::decision::statistical::{
    DecideRequest, QuestionKind, StatisticalOutcome,
};
use medulla_decision_types::decision::AbstainReason;
use medulla_graph_core::graph::GraphView;
use medulla_types::contract::BoundedVec;

use crate::decide::stat_executor::{Executed, ExecutionContext};
use crate::decide::stat_task_plan::Planned;

use super::DecisionFamily;

/// The family of the `task_plan` question kind.
pub struct TaskPlanFamily;

impl DecisionFamily for TaskPlanFamily {
    fn conclude(
        &self,
        ctx: &ExecutionContext,
        request: &DecideRequest,
        policy: &medulla_decision_types::decision::DecisionPolicy,
        graph_view: Option<&GraphView>,
    ) -> Option<Result<Executed, String>> {
        match crate::decide::stat_task_plan::task_plan(ctx, request, policy) {
            Err(detail) => Some(Err(detail)),
            // A plan that carries a `planning_admission` param is returned only
            // when the architecture view wires it (MEDULLA-DECISION-ENGINE-R171.3).
            Ok(Some(Planned::Plan(plan, _))) => {
                if let Err(detail) =
                    super::planning_admission::require_admitted(request, ctx.tenant_id, graph_view)
                {
                    return Some(Err(detail));
                }
                Some(Ok(executed(crate::decide::stat_task_plan::outcome(&plan))))
            }
            Ok(Some(Planned::Abstain { code, detail, .. })) => {
                Some(Ok(executed(StatisticalOutcome::Abstained {
                    reasons: BoundedVec::new(vec![AbstainReason::Family {
                        family: QuestionKind::TaskPlan,
                        code,
                        detail,
                    }])
                    .expect("one reason fits the bound"),
                })))
            }
            Ok(None) => None,
        }
    }
}

fn executed(outcome: StatisticalOutcome) -> Executed {
    Executed {
        matrix: None,
        ladder: LadderResult {
            outcome,
            calibration: None,
            explained: None,
            audit: None,
            logging: Vec::new(),
        },
        reading: None,
        seed: [0u8; 32],
        harness_contrast: None,
        topology: None,
        context_selection: None,
        thought: None,
        family_premises: Vec::new(),
        sealed_premises: Vec::new(),
        route: None,
        answered: None,
    }
}
