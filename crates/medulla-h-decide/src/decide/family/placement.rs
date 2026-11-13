//! The `placement` question family (MEDULLA-DECISION-ENGINE-R001.101,
//! MEDULLA-PLACEMENT-R001.7).
//!
//! The cheapest feasible deployment target for one workload. The family
//! concludes through `stat_placement`: the pure feasibility function over the
//! target facts and capacity headroom the server recorded in the question,
//! under the pinned policy's `placement.*` caps. Evaluate-only.

use medulla_decision_stats::decision::ladder::LadderResult;
use medulla_decision_types::decision::statistical::DecideRequest;
use medulla_decision_types::decision::{DecisionPolicy, PremiseRef};
use medulla_graph_core::graph::GraphView;

use crate::decide::stat_executor::{Executed, ExecutionContext};
use crate::decide::stat_placement::{placement_outcome, premises, question_of};

use super::DecisionFamily;

/// The family of the `placement` question kind.
pub struct PlacementFamily;

impl DecisionFamily for PlacementFamily {
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
        Some(placement_outcome(request, policy).map(|outcome| Executed {
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
            context_selection: None,
            family_premises: Vec::new(),
            route: None,
            answered: None,
        }))
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
