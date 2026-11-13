//! The gold-set guarantee of an evaluation (MEDULLA-DECISION-ENGINE-R032.2.3).
//!
//! A head that serves the `resolve_entity` pair decision acts only inside its
//! conformal prediction set, so its evaluation is held to that guarantee on a
//! gold-labelled set: the empirical coverage must reach `1 - alpha` over at
//! least `gold_min_pairs` pairs, both decision-policy knobs of the evaluation's
//! policy (`resolve_entity.alpha`, `resolve_entity.gold_min_pairs`). A shortfall
//! is named on the receipt as a failed gate, `GOLD_SET_TOO_SMALL` or
//! `CONFORMAL_COVERAGE_UNMET`, so the receipt fails and the head cannot be
//! published; the receipt records the alpha, the counts and the gold-set digest
//! it was held to.

use medulla_decision_stats::decision::evaluate::seal_receipt;
use medulla_decision_types::decision::jobs::{
    DecisionEvalReceipt, GoldGuarantee, GoldGuaranteeEvidence,
};
use medulla_decision_types::decision::knob_families::resolve_entity as knobs;
use medulla_decision_types::decision::statistical::StatisticalErrorCode;
use medulla_decision_types::decision::{DecisionPolicy, UnitRationalWire};
use medulla_types::contract::BoundedVec;

use super::stat_support::refusal;

/// Hold `receipt` to `guarantee`: record the evidence, name every unmet gate
/// in `failed_gates`, and seal the receipt again. A receipt with no full-label
/// metrics (a bandit evaluation) has no gold set to judge and is refused.
pub(super) fn apply(
    receipt: &mut DecisionEvalReceipt,
    guarantee: GoldGuarantee,
    policy: &DecisionPolicy,
    gold_set_digest: &str,
) -> Result<(), String> {
    let Some(metrics) = receipt.metrics else {
        return Err(refusal(
            StatisticalErrorCode::DatasetInvalid,
            "a gold-set guarantee needs a full-label evaluation (a gold_set_digest)",
        ));
    };
    let alpha_micros = knobs::alpha_micros(policy) as u64;
    let min_pairs = knobs::gold_min_pairs(policy) as u64;
    let alpha = UnitRationalWire::new(alpha_micros, 1_000_000)
        .map_err(|detail| refusal(StatisticalErrorCode::ParameterInvalid, detail))?;
    let mut failed: Vec<String> = receipt.failed_gates.as_slice().to_vec();
    if metrics.n_items < min_pairs {
        failed.push(StatisticalErrorCode::GoldSetTooSmall.as_str().to_string());
    } else if u128::from(metrics.covered) * 1_000_000
        < u128::from(metrics.n_items) * u128::from(1_000_000 - alpha_micros)
    {
        failed.push(
            StatisticalErrorCode::ConformalCoverageUnmet
                .as_str()
                .to_string(),
        );
    }
    receipt.gold_guarantee = Some(GoldGuaranteeEvidence {
        guarantee,
        alpha,
        n_gold: metrics.n_items,
        min_pairs,
        covered: metrics.covered,
        gold_set_digest: gold_set_digest.to_string(),
    });
    receipt.passed = failed.is_empty();
    receipt.failed_gates = BoundedVec::new(failed)
        .map_err(|detail| refusal(StatisticalErrorCode::ParameterInvalid, detail))?;
    seal_receipt(receipt);
    Ok(())
}
