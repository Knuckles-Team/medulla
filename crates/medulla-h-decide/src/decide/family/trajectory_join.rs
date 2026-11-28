//! The `trajectory_join` question family (MEDULLA-DECISION-ENGINE-R001.101,
//! R753.754).
//!
//! Joins competing trajectories into the strongest one with the pure `join`
//! kernel. The trajectories are the declared options, and their entries travel
//! as the typed `trajectory_join` parameter. The record rests on the weakest
//! evidence among the sources and resolves as an `Optimization`. The harness
//! applies the winner.

use medulla_decision_statistical_types::decision::statistical::declared::DeclaredOption;
use medulla_decision_statistical_types::decision::trajectory_verdict::JoinRequest;
use medulla_decision_stats::decision::trajectory_join::{join, MAX_JOIN_TRAJECTORIES};
use medulla_decision_types::decision::statistical::outcome::FamilyAnswer;
use medulla_graph_core::graph::GraphView;
use medulla_types::evidence_class::EvidenceClass;
use {
    medulla_decision_statistical_types::decision::statistical::errors::StatisticalErrorCode,
    medulla_decision_types::decision::statistical::DecideRequest,
    medulla_decision_types::decision::statistical::StatisticalOutcome,
    medulla_decision_types::decision::statistical::TypedValue,
};
use {
    medulla_decision_statistical_types::decision::DecisionPolicy,
    medulla_decision_types::decision::record::ResolutionKind,
};

use crate::decide::stat_executor::{Executed, ExecutionContext, FamilyExecuted};
use crate::decide::stat_support::invalid_parameter;

use super::kernel::{advisory, body, claim, evidence_label, shape};
use super::DecisionFamily;

const KIND: &str = "trajectory_join";
const PARAM: &str = "trajectory_join";

fn parse(request: &DecideRequest) -> Result<&JoinRequest, String> {
    let options = shape(request, KIND)?;
    let TypedValue::TrajectoryJoin(entries) = body(request, KIND, PARAM)? else {
        return Err(invalid_parameter(format!(
            "{KIND} parameter {PARAM} has the wrong type"
        )));
    };
    // A join over more than the bound refuses `CANDIDATE_SET_TOO_LARGE`
    // (MEDULLA-DECISION-ENGINE-R753.752). The declared candidates cannot name more
    // than `MAX_DECLARED_OPTIONS`, so the entry count is checked first: an
    // oversize join is refused for its size, not for failing to declare itself.
    if entries.entries.len() > MAX_JOIN_TRAJECTORIES {
        return Err(StatisticalErrorCode::CandidateSetTooLarge.refusal(format!(
            "a join takes at most {MAX_JOIN_TRAJECTORIES} trajectories, got {}",
            entries.entries.len()
        )));
    }
    let mut ids: Vec<&str> = entries
        .entries
        .iter()
        .map(|e| e.trajectory_id.as_str())
        .collect();
    ids.sort_unstable();
    let declared: Vec<&str> = options.iter().map(|o| o.option_id.as_str()).collect();
    if declared != ids {
        return Err(invalid_parameter(
            "the declared candidates must be exactly the sorted trajectory ids",
        ));
    }
    Ok(&**entries)
}

/// The family of the `trajectory_join` question kind.
pub struct TrajectoryJoinFamily;

impl DecisionFamily for TrajectoryJoinFamily {
    /// A join over no trajectory is `JOIN_EMPTY_SET`, raised before the generic
    /// declared-set check can answer `PARAMETER_INVALID`.
    fn precheck_declared(&self, options: &[DeclaredOption]) -> Result<(), String> {
        if options.is_empty() {
            return Err(
                StatisticalErrorCode::JoinEmptySet.refusal("a join needs at least one trajectory")
            );
        }
        Ok(())
    }

    fn constraints(&self, request: &DecideRequest) -> Result<(), String> {
        parse(request).map(|_| ())
    }

    fn conclude(
        &self,
        _ctx: &ExecutionContext,
        request: &DecideRequest,
        _policy: &DecisionPolicy,
        _graph_view: Option<&GraphView>,
    ) -> Option<Result<Executed, String>> {
        Some(parse(request).and_then(|join_request| {
            let distilled = join(join_request.entries.as_slice())?;
            let field = "params.trajectory_join";
            let mut sources: Vec<_> = join_request.entries.iter().collect();
            sources.sort_by(|a, b| a.trajectory_id.cmp(&b.trajectory_id));
            let mut premises: Vec<_> = sources
                .iter()
                .map(|entry| {
                    claim(
                        &entry.trajectory_id,
                        format!(
                            "join:source={}/{}/{}",
                            entry.trajectory_id,
                            evidence_label(entry.evidence),
                            entry.verifier_score.value
                        ),
                        field,
                    )
                })
                .collect();
            premises.push(claim(
                &distilled.winner_id,
                format!("join:winner={}", distilled.winner_id),
                field,
            ));
            premises.push(claim(
                &distilled.trajectory_id,
                format!("join:derivation={}", distilled.derivation_digest),
                field,
            ));
            // `Claim` is the weakest class, so the maximum is the weakest source.
            let weakest = join_request.entries.iter().map(|e| e.evidence).max();
            Ok(Executed::from_family(
                advisory(),
                Some(FamilyExecuted {
                    answer: Some(FamilyAnswer::TrajectoryJoin(Box::new(distilled))),
                    premises,
                    resolution: None,
                    evidence: weakest,
                }),
            ))
        }))
    }

    fn resolution_kind(&self, _outcome: &StatisticalOutcome) -> Option<ResolutionKind> {
        Some(ResolutionKind::Optimization)
    }

    fn evidence_class(&self, _outcome: &StatisticalOutcome) -> Option<EvidenceClass> {
        Some(EvidenceClass::Claim)
    }
}
