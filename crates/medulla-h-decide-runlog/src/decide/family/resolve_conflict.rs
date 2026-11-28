//! The `resolve_conflict` question family (MEDULLA-DECISION-ENGINE-R828).
//!
//! Stage 6 (statistical rank) scores both candidates of one branch-merge
//! conflict as `source_reliability x confidence`, in `f64`, and checks each
//! score is finite before any comparison. Stage 7 (conclude): the higher
//! score wins when the gap is at least `knob conflict.min_margin`; a smaller
//! gap is broken by the later `valid_from` only when the two validity windows
//! overlap and `knob conflict.recency_tiebreak` is 1; otherwise the decision
//! abstains with `CONFLICT_UNRESOLVED`. A request without a pinned policy
//! abstains with `CONFLICT_POLICY_MISSING`, a nonfinite score with
//! `CONFLICT_NONFINITE`. Evaluate-only: the `BranchMerge` handler, reached
//! through the `ConflictDecider` seam, writes the resolution.

use medulla_agent_types::agent_component::DecisionPolicyRef;
use medulla_graph_core::graph::GraphView;
use medulla_types::evidence_class::EvidenceClass;
use {
    medulla_decision_statistical_types::decision::record::premise::PremiseClass,
    medulla_decision_statistical_types::decision::record::premise::PremiseRef,
    medulla_decision_statistical_types::decision::DecisionPolicy,
    medulla_decision_types::decision::record::ResolutionKind,
};
use {
    medulla_decision_statistical_types::decision::resolve_conflict::min_margin,
    medulla_decision_statistical_types::decision::resolve_conflict::recency_tiebreak,
    medulla_decision_statistical_types::decision::resolve_conflict::ConflictRefusal,
    medulla_decision_statistical_types::decision::resolve_conflict::CONFLICT_NONFINITE,
    medulla_decision_statistical_types::decision::resolve_conflict::CONFLICT_OPTION,
    medulla_decision_statistical_types::decision::resolve_conflict::CONFLICT_POLICY_MISSING,
    medulla_decision_statistical_types::decision::resolve_conflict::CONFLICT_UNRESOLVED,
    medulla_decision_statistical_types::decision::resolve_conflict::PARAM_CONFLICT,
    medulla_decision_statistical_types::decision::resolve_conflict::WINNER_A,
    medulla_decision_statistical_types::decision::resolve_conflict::WINNER_B,
};
use {
    medulla_decision_statistical_types::decision::statistical::declared::ConflictCandidates,
    medulla_decision_statistical_types::decision::statistical::declared::DeclaredOption,
};
use {
    medulla_decision_statistical_types::decision::statistical::question_kind::QuestionKind,
    medulla_decision_types::decision::statistical::DecideRequest,
    medulla_decision_types::decision::statistical::StatisticalOutcome,
    medulla_decision_types::decision::statistical::TypedValue,
};

use {
    medulla_h_decide_base::decide::stat_executor::Executed,
    medulla_h_decide_base::decide::stat_executor::ExecutionContext,
    medulla_h_decide_base::decide::stat_executor::FamilyPremise,
    medulla_h_decide_base::decide::stat_executor::FamilyPremiseSource,
};

use medulla_h_decide_base::decide::family::DecisionFamily;
use {
    medulla_h_decide_base::decide::family::support::abstained,
    medulla_h_decide_base::decide::family::support::advisory,
    medulla_h_decide_base::decide::family::support::invalid,
    medulla_h_decide_base::decide::family::support::param,
    medulla_h_decide_base::decide::family::support::single_option,
};

/// The family of the `resolve_conflict` question kind.
pub struct ResolveConflictFamily;

fn candidates(request: &DecideRequest) -> Result<&ConflictCandidates, String> {
    match param(request, PARAM_CONFLICT)? {
        Some(TypedValue::ResolveConflict(pair)) => Ok(&**pair),
        Some(_) => Err(invalid(format!(
            "parameter {PARAM_CONFLICT} is not a resolve_conflict body"
        ))),
        None => Err(invalid(format!("parameter {PARAM_CONFLICT} is required"))),
    }
}

fn unit_range(pair: &ConflictCandidates) -> Result<(), String> {
    for (side, candidate) in [("a", &pair.candidate_a), ("b", &pair.candidate_b)] {
        for (field, value) in [
            ("source_reliability", candidate.source_reliability),
            ("confidence", candidate.confidence),
        ] {
            // A nonfinite input surfaces as a nonfinite score, refused in conclude.
            if value.is_finite() && !(0.0..=1.0).contains(&value) {
                return Err(invalid(format!(
                    "candidate_{side}.{field} must lie in [0, 1]"
                )));
            }
        }
    }
    Ok(())
}

/// The winner of `pair` under `policy`, or why there is none.
fn winner(
    pair: &ConflictCandidates,
    policy: &DecisionPolicy,
) -> Result<&'static str, ConflictRefusal> {
    let (score_a, score_b) = (pair.candidate_a.score(), pair.candidate_b.score());
    if !score_a.is_finite() || !score_b.is_finite() {
        return Err(ConflictRefusal::new(
            CONFLICT_NONFINITE,
            format!("conflict on {} has a nonfinite score", pair.entity),
        ));
    }
    let margin = min_margin(policy);
    if (score_a - score_b).abs() >= margin {
        return Ok(if score_a > score_b {
            WINNER_A
        } else {
            WINNER_B
        });
    }
    let (a, b) = (&pair.candidate_a, &pair.candidate_b);
    if recency_tiebreak(policy) && a.overlaps(b) && a.valid_from != b.valid_from {
        return Ok(if a.valid_from > b.valid_from {
            WINNER_A
        } else {
            WINNER_B
        });
    }
    Err(ConflictRefusal::new(
        CONFLICT_UNRESOLVED,
        format!(
            "conflict on {}: score gap {} is under the margin {margin}",
            pair.entity,
            (score_a - score_b).abs()
        ),
    ))
}

fn score_premises(pair: &ConflictCandidates) -> Vec<FamilyPremise> {
    [
        ("conflict:score_a", pair.candidate_a.score()),
        ("conflict:score_b", pair.candidate_b.score()),
    ]
    .into_iter()
    .map(|(name, score)| FamilyPremise {
        subject: "resolve_conflict".into(),
        fact: format!("{name}={score}"),
        class: PremiseClass::Claim,
        source: FamilyPremiseSource::Request(PARAM_CONFLICT),
    })
    .collect()
}

impl DecisionFamily for ResolveConflictFamily {
    fn validate_candidates(
        &self,
        options: &[DeclaredOption],
        _principal: &str,
    ) -> Result<(), String> {
        single_option(options, CONFLICT_OPTION)
    }

    fn constraints(&self, request: &DecideRequest) -> Result<(), String> {
        unit_range(candidates(request)?)
    }

    fn conclude(
        &self,
        _ctx: &ExecutionContext,
        request: &DecideRequest,
        policy: &DecisionPolicy,
        _graph_view: Option<&GraphView>,
    ) -> Option<Result<Executed, String>> {
        Some((|| {
            let pair = candidates(request)?;
            if matches!(request.policy, DecisionPolicyRef::Default) {
                let refused = ConflictRefusal::new(
                    CONFLICT_POLICY_MISSING,
                    "a conflict decision needs a pinned decision policy",
                );
                return Ok(abstained(QuestionKind::ResolveConflict, &refused));
            }
            let mut executed = match winner(pair, policy) {
                Ok(id) => advisory(vec![id.to_string()])?,
                Err(refused) => abstained(QuestionKind::ResolveConflict, &refused),
            };
            executed.family_premises = score_premises(pair);
            Ok(executed)
        })())
    }

    fn premises(
        &self,
        request: &DecideRequest,
        executed: &Executed,
        policy_digest: &str,
    ) -> Vec<PremiseRef> {
        if executed.family_premises.is_empty() {
            let Ok(pair) = candidates(request) else {
                return Vec::new();
            };
            return score_premises(pair)
                .iter()
                .map(|premise| premise.seal(policy_digest))
                .collect();
        }
        executed
            .family_premises
            .iter()
            .map(|premise| premise.seal(policy_digest))
            .collect()
    }

    fn resolution_kind(&self, outcome: &StatisticalOutcome) -> Option<ResolutionKind> {
        Some(match outcome {
            StatisticalOutcome::Abstained { .. } => ResolutionKind::Abstention,
            _ => ResolutionKind::Constraint,
        })
    }

    fn evidence_class(&self, _outcome: &StatisticalOutcome) -> Option<EvidenceClass> {
        Some(EvidenceClass::Claim)
    }
}
