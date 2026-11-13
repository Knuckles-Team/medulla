//! The `task_plan` question family (MEDULLA-DECISION-ENGINE-R001.101).
//!
//! Bounded explicit DAG planning: the family concludes a typed plan from
//! the typed `task_plan` parameter (evaluate-only; admission belongs to the
//! host).

use medulla_decision_stats::decision::ladder::LadderResult;
use medulla_decision_types::decision::statistical::DecideRequest;
use medulla_graph_core::graph::GraphView;

use crate::decide::stat_executor::{Executed, ExecutionContext};

use super::DecisionFamily;

/// The family of the `task_plan` question kind.
pub struct TaskPlanFamily;

impl DecisionFamily for TaskPlanFamily {
    fn conclude(
        &self,
        ctx: &ExecutionContext,
        request: &DecideRequest,
        policy: &medulla_decision_types::decision::DecisionPolicy,
        _graph_view: Option<&GraphView>,
    ) -> Option<Result<Executed, String>> {
        match crate::decide::stat_task_plan::task_plan(ctx, request, policy) {
            Err(detail) => Some(Err(detail)),
            Ok(Some(plan)) => Some(Ok(Executed {
                matrix: None,
                ladder: LadderResult {
                    outcome: crate::decide::stat_task_plan::outcome(&plan),
                    calibration: None,
                    explained: None,
                    audit: None,
                    logging: Vec::new(),
                },
                reading: None,
                seed: [0u8; 32],
                harness_contrast: None,
                topology: None,
                thought: None,
            })),
            Ok(None) => None,
        }
    }
}
