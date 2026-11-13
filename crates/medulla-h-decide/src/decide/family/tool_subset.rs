//! The `tool_subset` question family (MEDULLA-DECISION-ENGINE-R001.101).
//!
//! The smallest tool subset covering every required capability within a
//! context-token budget (MEDULLA-DECISION-ENGINE-R045): the family concludes
//! through `stat_tool_subset`.

use medulla_decision_stats::decision::ladder::LadderResult;
use medulla_decision_types::decision::statistical::DecideRequest;
use medulla_graph_core::graph::GraphView;

use crate::decide::stat_executor::{Executed, ExecutionContext};

use super::DecisionFamily;

/// The family of the `tool_subset` question kind.
pub struct ToolSubsetFamily;

impl DecisionFamily for ToolSubsetFamily {
    fn conclude(
        &self,
        _ctx: &ExecutionContext,
        request: &DecideRequest,
        _policy: &medulla_decision_types::decision::DecisionPolicy,
        _graph_view: Option<&GraphView>,
    ) -> Option<Result<Executed, String>> {
        let result = crate::decide::stat_tool_subset::tool_subset_outcome(request)?;
        Some(match result {
            Ok(outcome) => Ok(Executed {
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
                thought: None,
            }),
            Err(detail) => Err(detail),
        })
    }
}
