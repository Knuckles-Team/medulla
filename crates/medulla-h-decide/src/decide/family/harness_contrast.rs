//! The `harness_contrast` question family (MEDULLA-DECISION-ENGINE-R001.101).
//!
//! The contrastive harness composition decision (MEDULLA-DECISION-ENGINE-R803):
//! the family concludes through `stat_harness_contrast`.

use medulla_decision_types::decision::statistical::DecideRequest;
use medulla_graph_core::graph::GraphView;

use crate::decide::stat_executor::{Executed, ExecutionContext};

use super::DecisionFamily;

/// The family of the `harness_contrast` question kind.
pub struct HarnessContrastFamily;

impl DecisionFamily for HarnessContrastFamily {
    fn conclude(
        &self,
        _ctx: &ExecutionContext,
        request: &DecideRequest,
        graph_view: Option<&GraphView>,
    ) -> Option<Result<Executed, String>> {
        crate::decide::stat_harness_contrast::contrast_executed(request, graph_view)
    }
}
