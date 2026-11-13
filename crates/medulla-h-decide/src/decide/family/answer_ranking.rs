//! The `answer_ranking` question family (MEDULLA-DECISION-ENGINE-R134.106).
//!
//! Ranks answer candidates from the caller's pairwise claims through
//! `rank_answer_candidates`; the engine runs no verifier. The sealed record
//! carries each candidate's score, so a caller commits it through the decision
//! log and reads it back with `DecisionLogOp::Get`. `Decide` writes nothing.

use medulla_decision_outcome_types::decision::answer_ranking::{
    answer_ranking_premises, rank_answer_candidates,
};
use medulla_decision_types::decision::answer_ranking_request::AnswerRankingRequest;
use medulla_decision_types::decision::statistical::declared::DeclaredOption;
use medulla_decision_types::decision::statistical::{
    CandidateSource, DecideRequest, QuestionKind, ScoredOption, StatisticalOutcome, TypedValue,
};
use medulla_decision_types::decision::{DecisionPolicy, PremiseRef, QuantScaleTag, QuantisedValue};
use medulla_graph_core::graph::GraphView;
use medulla_types::contract::BoundedVec;

use crate::decide::stat_executor::{Executed, ExecutionContext};

use super::support::{abstained, advisory, invalid, param};
use super::DecisionFamily;

const PARAM_RANKING: &str = "answer_ranking";

/// The family of the `answer_ranking` question kind.
pub struct AnswerRankingFamily;

fn ranking_request(request: &DecideRequest) -> Result<&AnswerRankingRequest, String> {
    match param(request, PARAM_RANKING)? {
        Some(TypedValue::AnswerRanking(body)) => Ok(body),
        Some(_) => Err(invalid("parameter answer_ranking is not an answer ranking")),
        None => Err(invalid("parameter answer_ranking is required")),
    }
}

impl DecisionFamily for AnswerRankingFamily {
    /// The declared candidates must equal `candidate_ids`.
    fn validate_candidates(
        &self,
        options: &[DeclaredOption],
        _principal: &str,
    ) -> Result<(), String> {
        // The declared set is compared with `candidate_ids` in `constraints`,
        // which sees the ranking request.
        if options.is_empty() {
            return Err(invalid(
                "an answer_ranking question declares its candidates",
            ));
        }
        Ok(())
    }

    fn constraints(&self, request: &DecideRequest) -> Result<(), String> {
        let body = ranking_request(request)?;
        let mut declared: Vec<&str> = match &request.candidates {
            CandidateSource::Declared { options } => {
                options.iter().map(|o| o.option_id.as_str()).collect()
            }
            _ => {
                return Err(invalid(
                    "an answer_ranking question declares its candidates",
                ))
            }
        };
        let mut named: Vec<&str> = body.candidate_ids.iter().map(String::as_str).collect();
        declared.sort_unstable();
        named.sort_unstable();
        if declared != named {
            return Err(invalid(
                "declared candidates must equal the ranking candidate_ids",
            ));
        }
        Ok(())
    }

    fn conclude(
        &self,
        _ctx: &ExecutionContext,
        request: &DecideRequest,
        _policy: &DecisionPolicy,
        _graph_view: Option<&GraphView>,
    ) -> Option<Result<Executed, String>> {
        let body = match ranking_request(request) {
            Ok(body) => body,
            Err(detail) => return Some(Err(detail)),
        };
        Some(match rank_answer_candidates(body) {
            Err(refusal) => Ok(abstained(QuestionKind::AnswerRanking, &refusal)),
            Ok(ranking) => advisory(Vec::new()).and_then(|mut executed| {
                let scores: Vec<ScoredOption> = ranking
                    .global_order
                    .iter()
                    .map(|ranked| ScoredOption {
                        option_id: ranked.candidate_id.clone(),
                        score: QuantisedValue {
                            scale: QuantScaleTag::Q32,
                            value: i64::from(ranked.score) << 32,
                        },
                        probability: None,
                    })
                    .collect();
                let scores = BoundedVec::new(scores).map_err(invalid)?;
                executed.ladder.outcome = StatisticalOutcome::Advisory {
                    scores,
                    calibrated: false,
                };
                Ok(executed)
            }),
        })
    }

    fn premises(
        &self,
        request: &DecideRequest,
        _executed: &Executed,
        _policy_digest: &str,
    ) -> Vec<PremiseRef> {
        ranking_request(request)
            .ok()
            .and_then(|body| rank_answer_candidates(body).ok())
            .map(|ranking| answer_ranking_premises(&ranking))
            .unwrap_or_default()
    }
}
