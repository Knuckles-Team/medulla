//! The `team_coordination` question family (MEDULLA-DECISION-ENGINE-R170.1,
//! R170.2).
//!
//! Team and channel questions over the A2A team vocabulary. The signal model is
//! `medulla_decision_types::decision::team_coordination`; the fallback rule and
//! its team read are in `stat_team`. Exactly one legal option concludes with
//! `ResolutionKind::Constraint`. Evaluate-only: `Decide` writes nothing.

use medulla_decision_types::decision::statistical::declared::DeclaredOption;
use medulla_decision_types::decision::statistical::{DecideRequest, StatisticalOutcome};
use medulla_decision_types::decision::{DecisionPolicy, ResolutionKind};
use medulla_graph_core::graph::GraphView;

use crate::decide::stat_executor::{Executed, ExecutionContext};
use crate::decide::stat_team;

use super::DecisionFamily;

/// The refusal table of the family (MEDULLA-DECISION-ENGINE-R170.4), declared
/// beside the codes so the uniqueness test of the registry collects it.
pub use medulla_decision_types::decision::codes::TEAM_REFUSALS as REFUSALS;

/// The family of the `team_coordination` question kind.
pub struct TeamCoordinationFamily;

impl DecisionFamily for TeamCoordinationFamily {
    fn validate_candidates(
        &self,
        options: &[DeclaredOption],
        _principal: &str,
    ) -> Result<(), String> {
        stat_team::validate_options(options)
    }

    fn constraints(&self, request: &DecideRequest) -> Result<(), String> {
        stat_team::admit(request)
    }

    fn conclude(
        &self,
        ctx: &ExecutionContext,
        request: &DecideRequest,
        policy: &DecisionPolicy,
        graph_view: Option<&GraphView>,
    ) -> Option<Result<Executed, String>> {
        Some(stat_team::conclude(
            request,
            ctx.tenant_id,
            ctx.now_ms,
            policy,
            graph_view,
        ))
    }

    fn resolution_kind(&self, outcome: &StatisticalOutcome) -> Option<ResolutionKind> {
        matches!(outcome, StatisticalOutcome::Advisory { .. }).then_some(ResolutionKind::Constraint)
    }
}
