//! The held-out evidence of an evaluated program revision
//! (MEDULLA-DECISION-ENGINE-R810.7).
//!
//! The challenger score is the selected candidate's aggregate score on the
//! test split, the champion score is the request's baseline, and the
//! evaluation set is the sorted test-split example refs. The promotion gate
//! (`Decide` kind `Promotion`) reads the evidence off the stored
//! `ProgramRevision` node.

use medulla_decision_outcome_types::decision::promotion::{
    GraderKind, HeldOutEvidence, ReceiptKind, RevisionSubject, HELD_OUT_EVIDENCE_SCHEMA_VERSION,
};
use sha2::{Digest, Sha256};

use crate::{ExampleSplit, OptimizationRequest, OptimizationResult};

fn ppm(score: f64) -> i64 {
    (score.clamp(0.0, 1.0) * 1_000_000.0).round() as i64
}

/// The evidence of `result`'s selected candidate, `None` when no candidate was
/// selected or the selected candidate carries no evaluation.
pub fn program_held_out_evidence(
    request: &OptimizationRequest,
    result: &OptimizationResult,
) -> Option<HeldOutEvidence> {
    let candidate = result.selected_candidate()?;
    let evaluation = candidate.evaluation.as_ref()?;
    let mut tests: Vec<_> = request
        .corpus
        .examples
        .iter()
        .filter(|example| example.split == ExampleSplit::Test)
        .collect();
    tests.sort_by(|left, right| left.example_ref.as_str().cmp(right.example_ref.as_str()));
    let mut set = Sha256::new();
    set.update(b"eg-program.held-out-set.v1\0");
    for example in &tests {
        set.update((example.example_ref.as_str().len() as u64).to_be_bytes());
        set.update(example.example_ref.as_str().as_bytes());
    }
    let receipt = Sha256::digest(serde_json::to_vec(evaluation).ok()?);
    let challenger = ppm(evaluation.aggregate_score);
    let champion = ppm(request.baseline.aggregate_score);
    let judged = tests
        .iter()
        .any(|example| example.grader_kind == GraderKind::ModelJudge);
    Some(HeldOutEvidence {
        schema_version: HELD_OUT_EVIDENCE_SCHEMA_VERSION,
        subject: RevisionSubject::Program {
            program_ref: result.program_ref.as_str().to_owned(),
        },
        challenger_ref: candidate.candidate_ref.as_str().to_owned(),
        champion_ref: None,
        receipt_kind: ReceiptKind::ProgramEvaluation,
        receipt_digest: hex::encode(receipt),
        evaluation_set_digest: hex::encode(set.finalize()),
        challenger_score_ppm: challenger,
        champion_score_ppm: Some(champion),
        delta_ppm: challenger - champion,
        case_count: tests.len() as u64,
        independent: !tests.is_empty() && !judged,
        grader_kind: if judged {
            GraderKind::ModelJudge
        } else {
            tests
                .first()
                .map_or(GraderKind::TraceMetric, |example| example.grader_kind)
        },
        measurement: None,
        review: None,
    })
}
