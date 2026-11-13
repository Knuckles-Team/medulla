//! The `rank` question family (MEDULLA-DECISION-ENGINE-R001.101, R130.3-R130.5).
//!
//! The shared matrix ladder scores the options; this family then orders them
//! through `medulla_epistemic::rank_calibrated`. An option ranks on the
//! calibrated probability of the pinned head and on nothing else: an option
//! without one leaves the ranked list with `UNCALIBRATED_CANDIDATE`, and when
//! none stays the decision abstains. Options that share a code are separated
//! by the caller's item-level score.

use medulla_decision_stats::decision::candidate::CandidateView;
use medulla_decision_stats::decision::quant::unit_wire;
use medulla_decision_types::decision::codes::UNCALIBRATED_CANDIDATE;
use medulla_decision_types::decision::statistical::declared::DeclaredOption;
use medulla_decision_types::decision::statistical::{
    CandidateSource, DecideRequest, QuestionKind, ScoredOption, StatisticalOutcome,
};
use medulla_decision_types::decision::{
    AbstainReason, PremiseClass, PremiseProvenance, PremiseRef,
};
use medulla_epistemic::{
    rank_calibrated, CalibratedRankRules, CalibratedRanking, RankedCandidate, TieBreakPolicy,
};
use medulla_types::contract::BoundedVec;

use crate::decide::stat_executor::{Executed, Pinned};

use super::DecisionFamily;

/// Candidate text naming the code options may tie on; the option id when absent.
const CODE_KEY: &str = "rank.code";
/// Candidate number separating options that share a code; zero when absent.
const ITEM_SCORE_KEY: &str = "rank.item_score";
/// The evidence class every ranked option carries: its score is a head reading.
const EVIDENCE_CLASS: &str = "claim";
/// The tie-break policy the family ranks under.
const TIE_BREAK_POLICY: &str = "item_score";

/// The family of the `rank` question kind.
pub struct RankFamily;

/// A `Q32` value as a float, for ordering only.
fn q32_to_f64(q32: i64) -> f64 {
    q32 as f64 / (1_u64 << 32) as f64
}

/// The declared option `id`, when the request declares its options.
fn declared<'a>(request: Option<&'a DecideRequest>, id: &str) -> Option<&'a DeclaredOption> {
    let CandidateSource::Declared { options } = &request?.candidates else {
        return None;
    };
    options.iter().find(|option| option.option_id == id)
}

/// One ranking candidate per scored option.
fn candidates(request: Option<&DecideRequest>, scores: &[ScoredOption]) -> Vec<RankedCandidate> {
    scores
        .iter()
        .map(|scored| {
            let option = declared(request, &scored.option_id);
            RankedCandidate {
                id: scored.option_id.clone(),
                calibrated_score: scored
                    .probability
                    .map(|p| p.numerator() as f64 / p.denominator() as f64),
                code: option
                    .and_then(|o| o.texts.iter().find(|t| t.key == CODE_KEY))
                    .map(|t| t.text.clone())
                    .unwrap_or_else(|| scored.option_id.clone()),
                tie_break_score: option
                    .and_then(|o| o.numbers.iter().find(|n| n.key == ITEM_SCORE_KEY))
                    .map(|n| q32_to_f64(n.q32))
                    .unwrap_or(0.0),
                evidence_class: EVIDENCE_CLASS.to_string(),
            }
        })
        .collect()
}

fn rules() -> CalibratedRankRules {
    CalibratedRankRules {
        calibrated_first: true,
        tie_break: TieBreakPolicy::ItemScore,
    }
}

/// The advisory scores of `outcome` with the calibrated probability of each
/// option filled in from the head's reading, when the pinned head carries a
/// calibration. `None` for any other outcome.
fn calibrated_scores(pinned: &Pinned, executed: &Executed) -> Option<Vec<ScoredOption>> {
    let StatisticalOutcome::Advisory { scores, .. } = &executed.ladder.outcome else {
        return None;
    };
    let calibrated = pinned
        .head
        .as_ref()
        .is_some_and(|(head, _)| head.calibration.is_some());
    let probabilities = executed
        .reading
        .as_ref()
        .and_then(|reading| reading.probabilities.as_deref())
        .filter(|_| calibrated);
    Some(
        scores
            .iter()
            .enumerate()
            .map(|(index, scored)| {
                let mut scored = scored.clone();
                scored.probability = probabilities
                    .and_then(|p| p.get(index))
                    .and_then(|p| unit_wire(*p).ok());
                scored
            })
            .collect(),
    )
}

impl DecisionFamily for RankFamily {
    fn refine(
        &self,
        request: Option<&DecideRequest>,
        pinned: &Pinned,
        _views: &[CandidateView],
        _policy: &medulla_decision_types::decision::DecisionPolicy,
        _policy_digest: &str,
        mut executed: Executed,
    ) -> Result<Executed, String> {
        let Some(scores) = calibrated_scores(pinned, &executed) else {
            return Ok(executed);
        };
        let ranking = rank_calibrated(&candidates(request, &scores), rules());
        executed.ladder.outcome = if ranking.ranked.is_empty() {
            let reasons = ranking
                .abstained
                .iter()
                .map(|abstention| AbstainReason::Family {
                    family: QuestionKind::Rank,
                    code: UNCALIBRATED_CANDIDATE,
                    detail: abstention.id.clone(),
                })
                .collect();
            StatisticalOutcome::Abstained {
                reasons: BoundedVec::new(reasons)?,
            }
        } else {
            let ordered = ranking
                .ranked
                .iter()
                .filter_map(|(candidate, _)| scores.iter().find(|s| s.option_id == candidate.id))
                .cloned()
                .collect();
            StatisticalOutcome::Advisory {
                scores: BoundedVec::new(ordered)?,
                calibrated: true,
            }
        };
        Ok(executed)
    }

    fn premises(
        &self,
        request: &DecideRequest,
        executed: &Executed,
        policy_digest: &str,
    ) -> Vec<PremiseRef> {
        // A distilled ranking carries its scorer premises (R147.5) and no
        // calibrated head reading.
        if !executed.family_premises.is_empty() {
            return executed
                .family_premises
                .iter()
                .map(|premise| premise.seal(policy_digest))
                .collect();
        }
        let StatisticalOutcome::Advisory { scores, .. } = &executed.ladder.outcome else {
            return Vec::new();
        };
        let ranking: CalibratedRanking =
            rank_calibrated(&candidates(Some(request), scores.as_slice()), rules());
        let evaluation = PremiseProvenance::Observation {
            evaluation_id: request
                .head
                .as_ref()
                .map(|pin| pin.definition_digest.clone())
                .unwrap_or_default(),
        };
        let mut out = Vec::new();
        for (candidate, tie_break_applied) in &ranking.ranked {
            out.push(PremiseRef {
                subject: candidate.id.clone(),
                fact: format!(
                    "calibrated_score={}",
                    candidate.calibrated_score.unwrap_or_default()
                ),
                class: PremiseClass::Claim,
                provenance: evaluation.clone(),
            });
            out.push(PremiseRef {
                subject: candidate.id.clone(),
                fact: format!("tie_break_applied={tie_break_applied}"),
                class: PremiseClass::Claim,
                provenance: evaluation.clone(),
            });
        }
        out.push(PremiseRef {
            subject: "policy".to_string(),
            fact: format!("tie_break_policy={TIE_BREAK_POLICY}"),
            class: PremiseClass::Definition,
            provenance: PremiseProvenance::Policy {
                policy_digest: policy_digest.to_string(),
            },
        });
        out
    }
}
