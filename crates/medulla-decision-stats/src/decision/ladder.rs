//! Step 4 of the resolution ladder: the statistical act/abstain rule.
//!
//! A head may make the engine ACT only when all of this holds: it is a
//! full-label listwise head (linear or the resident scorer), it carries a
//! non-synthetic calibration with a certified act threshold and risk statement, its
//! calibration sample reaches the policy's `n_min`, its alpha, epsilon and
//! delta are no looser than the policy's, and -- for the state at hand -- the
//! act rule holds ([`act_rule`]: a conformal claim, the top probability at the
//! threshold, the top option inside the prediction set).
//! Anything less is at most an advisory score labelled uncalibrated -- and
//! only when the policy's cold-start mode allows advisory output at all.
//! Propensities are the executed policy's, never the head's mass.

use medulla_decision_types::{
    decision::statistical::head::DecisionHeadBody, decision::statistical::head::FittedRegime,
    decision::statistical::head::HeadCalibration,
};
use medulla_decision_types::{
    decision::statistical::AuditDraw, decision::statistical::CalibrationStatement,
    decision::statistical::RiskStatement, decision::statistical::ScoredOption,
    decision::statistical::StatisticalErrorCode, decision::statistical::StatisticalOutcome,
};
use medulla_decision_types::{
    decision::AbstainReason, decision::ColdStart, decision::DecisionPolicy,
    decision::StatisticalPolicy, decision::UnitRationalWire,
};
use medulla_types::contract::BoundedVec;

use super::exploration::{audit, logging_vector, plan, ExplorationPermit, ExplorationPlan};
use super::head_eval::{act_rule, calibration_statement, top_index, Evaluated};
use super::quant::{exact_wire, q32};
use super::refusal::{Refusal, RefusalResult};

/// What the rule reads.
#[derive(Debug, Clone, Copy)]
pub struct LadderInputs<'a> {
    /// Sorted by id; every index below refers to this order.
    pub candidate_ids: &'a [String],
    pub head: Option<&'a DecisionHeadBody>,
    pub reading: Option<&'a Evaluated>,
    pub policy: &'a DecisionPolicy,
    pub statistical: &'a StatisticalPolicy,
    pub permit: ExplorationPermit,
    pub seed: &'a [u8; 32],
}

/// What the rule concluded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LadderResult {
    pub outcome: StatisticalOutcome,
    pub calibration: Option<CalibrationStatement>,
    /// The option whose logit contributions the record explains.
    pub explained: Option<usize>,
    pub audit: Option<AuditDraw>,
    /// The executed policy's distribution (see [`logging_vector`]); empty when
    /// nothing was executed.
    pub logging: Vec<UnitRationalWire>,
}

fn budget(permit: ExplorationPermit) -> Option<UnitRationalWire> {
    match permit {
        ExplorationPermit::Off => None,
        ExplorationPermit::Budget(fraction) => Some(fraction),
    }
}

fn no_looser(candidate: UnitRationalWire, bound: UnitRationalWire) -> bool {
    u128::from(candidate.numerator()) * u128::from(bound.denominator())
        <= u128::from(bound.numerator()) * u128::from(candidate.denominator())
}

fn acting_head_eligible(head: &DecisionHeadBody) -> bool {
    head.kind.is_listwise() && head.regime == FittedRegime::FullLabel && !head.synthetic
}

fn calibration_permits_acting(
    calibration: &HeadCalibration,
    risk: RiskStatement,
    policy: &StatisticalPolicy,
) -> bool {
    !calibration.synthetic
        && calibration.n_calibration >= policy.n_min
        && no_looser(calibration.alpha, policy.alpha)
        && no_looser(risk.epsilon, policy.epsilon)
        && no_looser(risk.delta, policy.delta)
}

/// The calibration that licenses acting, if the head has one the policy accepts.
fn acting_calibration<'a>(
    head: &'a DecisionHeadBody,
    policy: &StatisticalPolicy,
) -> Option<(&'a HeadCalibration, RiskStatement)> {
    if !acting_head_eligible(head) {
        return None;
    }
    let calibration = head.calibration.as_ref()?;
    let risk = calibration.risk?;
    calibration.act_threshold?;
    calibration_permits_acting(calibration, risk, policy).then_some((calibration, risk))
}

fn abstained() -> StatisticalOutcome {
    StatisticalOutcome::Abstained {
        reasons: BoundedVec::new(vec![AbstainReason::InsufficientConfidence])
            .expect("one reason is inside the bound"),
    }
}

fn ids(inputs: &LadderInputs, indices: &[usize]) -> RefusalResult<BoundedVec<String, 64>> {
    BoundedVec::new(
        indices
            .iter()
            .map(|&i| inputs.candidate_ids[i].clone())
            .collect(),
    )
    .map_err(|detail| Refusal::new(StatisticalErrorCode::CandidateSetTooLarge, detail))
}

fn advisory(inputs: &LadderInputs, reading: &Evaluated) -> RefusalResult<StatisticalOutcome> {
    let scores = inputs
        .candidate_ids
        .iter()
        .zip(&reading.logits)
        .map(|(id, logit)| {
            Ok(ScoredOption {
                option_id: id.clone(),
                score: q32(*logit)?,
                probability: None,
            })
        })
        .collect::<RefusalResult<Vec<_>>>()?;
    Ok(StatisticalOutcome::advisory(BoundedVec::new(scores)
            .map_err(|detail| Refusal::new(StatisticalErrorCode::CandidateSetTooLarge, detail))?, false))
}

fn greedy(reading: Option<&Evaluated>) -> Option<usize> {
    let reading = reading?;
    top_index(reading.probabilities.as_deref().unwrap_or(&reading.logits))
}

fn explored(
    inputs: &LadderInputs,
    drawn: &ExplorationPlan,
    greedy_choice: Option<usize>,
) -> RefusalResult<LadderResult> {
    Ok(LadderResult {
        outcome: StatisticalOutcome::Explored {
            option_id: inputs.candidate_ids[drawn.chosen].clone(),
            propensity: drawn.propensity,
        },
        calibration: None,
        explained: None,
        audit: None,
        logging: logging_vector(
            inputs.candidate_ids.len(),
            greedy_choice,
            budget(inputs.permit),
        )?,
    })
}

fn exploration(
    inputs: &LadderInputs,
    greedy_choice: Option<usize>,
) -> RefusalResult<Option<ExplorationPlan>> {
    match inputs.permit {
        ExplorationPermit::Off => Ok(None),
        ExplorationPermit::Budget(fraction) => plan(
            inputs.seed,
            fraction,
            inputs.candidate_ids.len(),
            greedy_choice,
        ),
    }
}

fn act_or_abstain(
    inputs: &LadderInputs,
    reading: &Evaluated,
    calibration: &HeadCalibration,
    risk: RiskStatement,
    propensity: UnitRationalWire,
) -> RefusalResult<LadderResult> {
    let probabilities = reading.probabilities.as_deref().unwrap_or(&reading.logits);
    let rule = act_rule(probabilities, calibration);
    let (acts, top) = (rule.acts, rule.top);
    let logging = match (acts, top) {
        (true, Some(top)) => {
            logging_vector(inputs.candidate_ids.len(), Some(top), budget(inputs.permit))?
        }
        _ => Vec::new(),
    };
    let (outcome, explained, audit_draw) = match (acts, top) {
        (true, Some(top)) => (
            StatisticalOutcome::Acted {
                option_id: inputs.candidate_ids[top].clone(),
                propensity,
                prediction_set: ids(inputs, &rule.prediction_set)?,
                risk,
            },
            Some(top),
            Some(audit(inputs.seed, inputs.statistical.audit_sample)),
        ),
        _ => (abstained(), top, None),
    };
    Ok(LadderResult {
        outcome,
        calibration: Some(calibration_statement(calibration)),
        explained,
        audit: audit_draw,
        logging,
    })
}

fn uncalibrated(inputs: &LadderInputs, reading: Option<&Evaluated>) -> RefusalResult<LadderResult> {
    let advisory_allowed = !matches!(inputs.policy.cold_start, ColdStart::DeterministicOnly);
    let outcome = match (advisory_allowed, reading) {
        (true, Some(reading)) => advisory(inputs, reading)?,
        _ => abstained(),
    };
    Ok(LadderResult {
        outcome,
        calibration: None,
        explained: greedy(reading),
        audit: None,
        logging: Vec::new(),
    })
}

/// Run the act/abstain rule.
pub fn decide(inputs: &LadderInputs) -> RefusalResult<LadderResult> {
    let acting = inputs
        .head
        .and_then(|head| acting_calibration(head, inputs.statistical));
    let greedy_choice = greedy(inputs.reading);
    let drawn = exploration(inputs, greedy_choice)?;
    if let Some(drawn) = drawn.filter(|d| d.explored) {
        return explored(inputs, &drawn, greedy_choice);
    }
    let propensity = match drawn {
        Some(d) => d.propensity,
        None => exact_wire(1, 1)?,
    };
    match (acting, inputs.reading) {
        (Some((calibration, risk)), Some(reading)) => {
            act_or_abstain(inputs, reading, calibration, risk, propensity)
        }
        _ => uncalibrated(inputs, inputs.reading),
    }
}

/// What [`cost_tie_break`] concluded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CostTie {
    /// Fewer than two options lie within epsilon of the best score: there is
    /// nothing to break and the scores stand.
    NoTie,
    /// The tie set and the option whose observed cost is lowest (equal cost
    /// breaks on option id ascending).
    Chosen {
        option_id: String,
        /// Every tie-set option with its observed cost in micro-units, in id
        /// order.
        tie_set: Vec<(String, u64)>,
    },
    /// A tie-set option has no observed cost; the first such option in id
    /// order. Unknown is never read as zero.
    MissingCost { option_id: String },
}

/// The ids of the options whose score lies within `epsilon_q32` of the best,
/// in id order.
pub fn tie_ids(scored: &[ScoredOption], epsilon_q32: i64) -> Vec<&str> {
    let Some(best) = scored.iter().map(|option| option.score.value).max() else {
        return Vec::new();
    };
    let floor = best.saturating_sub(epsilon_q32.max(0));
    let mut tied: Vec<&str> = scored
        .iter()
        .filter(|option| option.score.value >= floor)
        .map(|option| option.option_id.as_str())
        .collect();
    tied.sort_unstable();
    tied
}

/// Among options whose score lies within `epsilon_q32` of the best, prefer
/// the one with the lowest observed cost (R039.2). Scores and epsilon are
/// `Q32` integers, so the rule is exact. `costs` maps an option id to its
/// observed cost; an absent id has none.
pub fn cost_tie_break(
    scored: &[ScoredOption],
    costs: &std::collections::BTreeMap<String, u64>,
    epsilon_q32: i64,
) -> CostTie {
    let tied = tie_ids(scored, epsilon_q32);
    if tied.len() < 2 {
        return CostTie::NoTie;
    }
    let mut tie_set = Vec::with_capacity(tied.len());
    for id in tied {
        match costs.get(id) {
            Some(cost) => tie_set.push((id.to_string(), *cost)),
            None => {
                return CostTie::MissingCost {
                    option_id: id.to_string(),
                }
            }
        }
    }
    // The set is in id order, so the first minimum is the lowest id.
    let option_id = tie_set
        .iter()
        .min_by_key(|(_, cost)| *cost)
        .map(|(id, _)| id.clone())
        .expect("a tie set has at least two options");
    CostTie::Chosen { option_id, tie_set }
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_decision_types::decision::{QuantScaleTag, QuantisedValue};
    use std::collections::BTreeMap;

    const ONE: i64 = 1 << 32;

    fn scored(id: &str, value: i64) -> ScoredOption {
        ScoredOption {
            option_id: id.to_string(),
            score: QuantisedValue {
                scale: QuantScaleTag::Q32,
                value,
            },
            probability: None,
        }
    }

    fn costs(rows: &[(&str, u64)]) -> BTreeMap<String, u64> {
        rows.iter().map(|(id, c)| (id.to_string(), *c)).collect()
    }

    // spec: MEDULLA-DECISION-ENGINE-R039.2
    #[test]
    fn cost_breaks_ties_within_epsilon_only() {
        let epsilon = ONE / 50; // 0.02
        let options = [
            scored("a", ONE * 9 / 10),
            scored("b", ONE * 9 / 10 - epsilon),
            scored("c", ONE * 9 / 10 - epsilon - 1),
        ];
        // c is the cheapest but lies one step outside the tie set; b is
        // exactly on the boundary and belongs to it.
        let all = costs(&[("a", 500), ("b", 300), ("c", 1)]);
        let CostTie::Chosen { option_id, tie_set } = cost_tie_break(&options, &all, epsilon) else {
            panic!("a tie of two options chooses")
        };
        assert_eq!(option_id, "b");
        assert_eq!(
            tie_set,
            vec![("a".to_string(), 500), ("b".to_string(), 300)]
        );

        // Equal cost breaks on option id ascending.
        let equal = costs(&[("a", 300), ("b", 300), ("c", 1)]);
        let CostTie::Chosen { option_id, .. } = cost_tie_break(&options, &equal, epsilon) else {
            panic!("chooses")
        };
        assert_eq!(option_id, "a");

        // Epsilon zero ties only exact equals: the best score stands alone.
        assert_eq!(cost_tie_break(&options, &all, 0), CostTie::NoTie);
        // A tie-set option without a cost is named; a cost outside the set is
        // not needed.
        let missing = costs(&[("a", 500), ("c", 1)]);
        assert_eq!(
            cost_tie_break(&options, &missing, epsilon),
            CostTie::MissingCost {
                option_id: "b".to_string()
            }
        );
        let outside = costs(&[("a", 500), ("b", 300)]);
        assert!(matches!(
            cost_tie_break(&options, &outside, epsilon),
            CostTie::Chosen { .. }
        ));
        // No options, no tie.
        assert_eq!(cost_tie_break(&[], &all, epsilon), CostTie::NoTie);
    }
}
