//! The `edit_concentration` question family (MEDULLA-DECISION-ENGINE-R173.3).
//!
//! Concludes through `concentration_verdict` over the harness edits of the
//! caller's filtered graph (`stat_concentration`). Evaluate-only: `Decide`
//! writes nothing.

use medulla_decision_types::decision::statistical::DecideRequest;
use medulla_graph_core::graph::GraphView;

use crate::decide::stat_concentration;
use crate::decide::stat_executor::{Executed, ExecutionContext};

use super::DecisionFamily;

/// The family of the `edit_concentration` question kind.
pub struct EditConcentrationFamily;

impl DecisionFamily for EditConcentrationFamily {
    fn conclude(
        &self,
        ctx: &ExecutionContext,
        request: &DecideRequest,
        _policy: &medulla_decision_types::decision::DecisionPolicy,
        graph_view: Option<&GraphView>,
    ) -> Option<Result<Executed, String>> {
        Some(stat_concentration::conclude(
            request,
            ctx.tenant_id,
            graph_view,
        ))
    }
}
