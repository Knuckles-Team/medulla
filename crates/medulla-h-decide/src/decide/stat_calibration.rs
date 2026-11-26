//! The rerank calibration head (MEDULLA-DECISION-ENGINE-R822): a fit of one
//! scorer version's raw scores against judged relevance, stored as an isotonic
//! curve and served only after a champion pointer move.
//!
//! A fit reads ONLY independently judged successful runs that recorded the
//! scorer version's raw scores. A returned unit is relevant when the answer
//! cited it. The curve is the pool-adjacent-violators solution over integer
//! parts-per-million pairs, reduced to at most
//! [`MAX_CALIBRATION_KNOTS`] knots by pooling the closest neighbours. Its pointer
//! is the champion of the `rerank_calibration` revision subject (R805).

use medulla_decision_stats::decision::retrieval::Verdict;
use medulla_decision_types::decision::codes::CALIBRATION_TOO_FEW_PAIRS;
use medulla_decision_types::decision::held_out::RevisionSubject;
use medulla_decision_types::decision::statistical::body::{
    canonical_body_bytes, content_digest_of,
};
use medulla_decision_types::decision::statistical::retrieval_calibration::{
    CalibrationCurve, CalibrationFitRequest, CalibrationFitted, CalibrationKnot,
    CalibrationReceipt, CALIBRATION_SCHEMA_VERSION, MAX_CALIBRATION_KNOTS, PPM,
};
use medulla_decision_types::decision::statistical::StatisticalErrorCode;
use medulla_persistence_records::agent_library::AgentLibraryStore;
use medulla_persistence_records::decision_jobs::{
    calibration_key, encode_artifact, read_calibration,
};
use medulla_types::contract::BoundedVec;

use super::stat_executor::ExecutionContext;
use super::stat_log::LogReader;
use super::stat_pointer::Qualified;
use super::stat_retrieval::{judged_outcomes, Scope};
use super::stat_support::{invalid_parameter as invalid, refusal};

/// The pointer key of one scorer version's calibration champion.
pub(super) fn calibration_pointer(scorer_version: &str) -> String {
    RevisionSubject::RerankCalibration {
        scorer_version: scorer_version.to_string(),
    }
    .pointer_key()
}

/// One block of pooled pairs: how many, how relevant, and the score sum.
#[derive(Clone, Copy)]
struct Block {
    score_sum: u128,
    count: u64,
    relevant: u64,
}

impl Block {
    fn merged(self, next: Block) -> Block {
        Block {
            score_sum: self.score_sum + next.score_sum,
            count: self.count + next.count,
            relevant: self.relevant + next.relevant,
        }
    }

    /// Relevance rate in ppm.
    fn rate_ppm(&self) -> u64 {
        self.relevant * u64::from(PPM) / self.count
    }

    /// True when `self`'s rate is strictly above `next`'s (a violation).
    fn above(&self, next: &Block) -> bool {
        u128::from(self.relevant) * u128::from(next.count)
            > u128::from(next.relevant) * u128::from(self.count)
    }
}

/// The isotonic knots of `(raw score ppm, relevant)` pairs: pool-adjacent-
/// violators over the distinct scores, then at most 64 knots.
pub(crate) fn fit_isotonic(pairs: &[(u32, bool)]) -> Vec<CalibrationKnot> {
    let mut sorted = pairs.to_vec();
    sorted.sort_unstable_by_key(|(score, _)| *score);
    let mut distinct: Vec<(u32, Block)> = Vec::new();
    for (score, relevant) in sorted {
        match distinct.last_mut() {
            Some((last, block)) if *last == score => {
                block.count += 1;
                block.relevant += u64::from(relevant);
                block.score_sum += u128::from(score);
            }
            _ => distinct.push((
                score,
                Block {
                    score_sum: u128::from(score),
                    count: 1,
                    relevant: u64::from(relevant),
                },
            )),
        }
    }
    let mut blocks: Vec<Block> = Vec::new();
    for (_, block) in distinct {
        let mut current = block;
        while let Some(previous) = blocks.last().copied() {
            if !previous.above(&current) {
                break;
            }
            blocks.pop();
            current = previous.merged(current);
        }
        blocks.push(current);
    }
    while blocks.len() > MAX_CALIBRATION_KNOTS {
        let closest = (0..blocks.len() - 1)
            .min_by_key(|i| blocks[i + 1].rate_ppm() - blocks[*i].rate_ppm())
            .unwrap_or(0);
        let next = blocks.remove(closest + 1);
        blocks[closest] = blocks[closest].merged(next);
    }
    blocks
        .iter()
        .map(|block| CalibrationKnot {
            score_ppm: u32::try_from(block.score_sum / u128::from(block.count)).unwrap_or(PPM),
            relevant_ppm: u32::try_from(block.rate_ppm()).unwrap_or(PPM),
        })
        .collect()
}

/// The judged `(raw score, relevant)` pairs of one scorer version.
fn judged_pairs(
    store: &AgentLibraryStore,
    reader: &LogReader,
    request: &CalibrationFitRequest,
) -> Result<Vec<(u32, bool)>, String> {
    let scope = Scope {
        question_id: request.question_id.as_deref(),
        window: request.window,
    };
    let mut pairs = Vec::new();
    for judged in judged_outcomes(store, reader, scope)? {
        let outcome = &judged.stored.outcome;
        if judged.verdict != Verdict::Success
            || outcome.scorer_version.as_deref() != Some(request.scorer_version.as_str())
        {
            continue;
        }
        for unit in outcome.returned.iter() {
            let Some(score) = unit.raw_score_ppm else {
                continue;
            };
            pairs.push((
                score,
                outcome.cited.iter().any(|id| *id == unit.evidence_id),
            ));
        }
    }
    Ok(pairs)
}

/// Fit, receipt and store the calibration curve of one scorer version under
/// the served tenant `tenant`.
pub(super) fn fit_calibration(
    ctx: &ExecutionContext,
    reader: &LogReader,
    request: &CalibrationFitRequest,
    tenant: &str,
) -> Result<CalibrationFitted, String> {
    if request.scorer_version.is_empty() {
        return Err(invalid("a calibration fit names its scorer version"));
    }
    let pairs = judged_pairs(ctx.store, reader, request)?;
    let floor = medulla_server_api::knob_config::knobs().retrieval_calibration_min_pairs;
    if (pairs.len() as u64) < floor {
        return Err(format!(
            "{}: {} judged pairs of scorer {} is fewer than the {floor} a fit needs",
            CALIBRATION_TOO_FEW_PAIRS.as_str(),
            pairs.len(),
            request.scorer_version
        ));
    }
    let curve = CalibrationCurve {
        schema_version: CALIBRATION_SCHEMA_VERSION,
        scorer_version: request.scorer_version.clone(),
        knots: BoundedVec::new(fit_isotonic(&pairs))?,
    };
    let curve_digest = content_digest_of(&canonical_body_bytes(&curve)?);
    let receipt = CalibrationReceipt {
        curve_digest: curve_digest.clone(),
        scorer_version: request.scorer_version.clone(),
        n_pairs: pairs.len() as u64,
        n_relevant: pairs.iter().filter(|(_, relevant)| *relevant).count() as u64,
        passed: curve.is_monotone(),
    };
    let fitted = CalibrationFitted {
        receipt_digest: content_digest_of(&canonical_body_bytes(&receipt)?),
        curve_digest,
        curve,
        receipt,
    };
    ctx.store.put_decision_artifacts(
        tenant,
        &[(
            calibration_key(&fitted.curve_digest),
            encode_artifact(&fitted)?,
        )],
    )?;
    Ok(fitted)
}

/// A fitted curve whose passing receipt is the named one, fitted for
/// `scorer_version`.
pub(super) fn qualified_calibration(
    store: &AgentLibraryStore,
    tenant: &str,
    scorer_version: &str,
    curve_digest: &str,
    receipt_digest: &str,
) -> Result<Qualified, String> {
    let mismatch = |detail: &str| refusal(StatisticalErrorCode::EvaluationReceiptMismatch, detail);
    let fitted = read_calibration(store, tenant, curve_digest)?
        .ok_or_else(|| invalid("no fitted calibration curve with that digest"))?;
    if fitted.receipt_digest != receipt_digest || !fitted.receipt.passed {
        return Err(mismatch(
            "the curve's receipt is not the named one, or did not pass",
        ));
    }
    if fitted.curve.scorer_version != scorer_version {
        return Err(mismatch("the curve was fitted for another scorer version"));
    }
    Ok(Qualified {
        target: curve_digest.to_string(),
        receipt_digest: receipt_digest.to_string(),
    })
}
