//! The `thought_search` question family (MEDULLA-DECISION-ENGINE-R001.101,
//! R750.754).
//!
//! Bounded multi-step reasoning search: the family concludes which frontier
//! leaves to expand next, or a stop with the best-so-far node, from the typed
//! `thought_search` parameter, the pinned policy's strategy and the
//! `thought_search.*` knobs. Evaluate-only. Scores are caller claims, so the
//! record is a `Claim` resolved as an `Optimization`.

use medulla_decision_stats::decision::ladder::LadderResult;
use medulla_decision_types::decision::statistical::{DecideRequest, StatisticalOutcome};
use medulla_decision_types::decision::{DecisionPolicy, EvidenceClass, ResolutionKind};
use medulla_graph_core::graph::GraphView;

use crate::decide::stat_executor::{Executed, ExecutionContext};

use super::DecisionFamily;

/// The family of the `thought_search` question kind.
pub struct ThoughtSearchFamily;

impl DecisionFamily for ThoughtSearchFamily {
    fn constraints(&self, request: &DecideRequest) -> Result<(), String> {
        crate::decide::stat_thought_search::parse(request).map(|_| ())
    }

    fn conclude(
        &self,
        _ctx: &ExecutionContext,
        request: &DecideRequest,
        policy: &DecisionPolicy,
        _graph_view: Option<&GraphView>,
    ) -> Option<Result<Executed, String>> {
        Some(
            crate::decide::stat_thought_search::conclude(request, policy).map(
                |(outcome, thought)| Executed {
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
                    thought,
                    family_premises: Vec::new(),
                    route: None,
                },
            ),
        )
    }

    fn resolution_kind(&self, _outcome: &StatisticalOutcome) -> Option<ResolutionKind> {
        Some(ResolutionKind::Optimization)
    }

    fn evidence_class(&self, _outcome: &StatisticalOutcome) -> Option<EvidenceClass> {
        Some(EvidenceClass::Claim)
    }
}
