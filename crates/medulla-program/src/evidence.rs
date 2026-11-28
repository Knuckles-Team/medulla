//! The held-out evidence of an evaluated program revision
//! (MEDULLA-DECISION-ENGINE-R810.7).
//!
//! The challenger score is the selected candidate's aggregate score on the
//! test split, the champion score is the request's baseline, and the
//! evaluation set is the sorted test-split example refs, digested by
//! `solve_scalar::digest_text`. The promotion gate
//! (`Decide` kind `Promotion`) reads the evidence off the stored
//! `ProgramRevision` node.

use medulla_query_types::solve_scalar::digest_text;
use sha2::{Digest, Sha256};
use {
    medulla_decision_policy_types::decision::held_out::GraderKind,
    medulla_decision_policy_types::decision::held_out::HeldOutEvidence,
    medulla_decision_policy_types::decision::held_out::ReceiptKind,
    medulla_decision_policy_types::decision::held_out::RevisionSubject,
    medulla_decision_policy_types::decision::held_out::HELD_OUT_EVIDENCE_SCHEMA_VERSION,
};

use crate::{ExampleSplit, OptimizationRequest, OptimizationResult, TrainingExample};

/// Domain of the evaluation-set digest: the sorted test-split example refs.
pub const HELD_OUT_SET_DIGEST_DOMAIN: &str = "eg-program.held-out-set.v1";

fn ppm(score: f64) -> i64 {
    (score.clamp(0.0, 1.0) * 1_000_000.0).round() as i64
}

/// Why a corpus cannot hold out evidence for a program revision
/// (MEDULLA-DECISION-ENGINE-R804.4, R810.4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeldOutRefusal {
    /// The corpus has no test-split example.
    Missing,
    /// The test split is non-empty but no example's grader may decide a
    /// promotion (`GraderKind::decides_promotion()`).
    NotIndependent,
}

/// The test-split examples whose grader decides a promotion: the independent
/// case count of the held-out evidence.
pub fn independent_case_count(examples: &[TrainingExample]) -> usize {
    examples
        .iter()
        .filter(|example| {
            example.split == ExampleSplit::Test && example.grader_kind.decides_promotion()
        })
        .count()
}

/// The one held-out rule: a corpus needs a test split, and at least one test
/// example must be graded by a kind that decides promotion. The served job
/// applies it at submit, so a refusal stores no job and stages no revision.
pub fn check_held_out_split(examples: &[TrainingExample]) -> Result<(), HeldOutRefusal> {
    if !examples
        .iter()
        .any(|example| example.split == ExampleSplit::Test)
    {
        return Err(HeldOutRefusal::Missing);
    }
    if independent_case_count(examples) == 0 {
        return Err(HeldOutRefusal::NotIndependent);
    }
    Ok(())
}

/// Why the evaluations of a reflective job cannot form a front
/// (MEDULLA-DECISION-ENGINE-R800.4).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FrontRefusal {
    /// The test split holds fewer examples than `knob front.min_eval_examples`.
    EvalTooSmall {
        test_examples: usize,
        minimum: usize,
    },
    /// An evaluation carries no `benchmark_score`, so it has one objective
    /// where the front needs two.
    ScoreLength { subject_ref: String },
}

/// The inputs a reflective (`Gepa`) job needs before it builds a front: when
/// the request carries candidate evaluations, its test split holds at least
/// `min_eval_examples` examples and every evaluation names both objectives. Any
/// other optimizer, and a draft with no evaluation, build no front and are not
/// checked. The served job applies it at submit beside [`check_held_out_split`],
/// so a refusal stores no job and stages no result.
pub fn check_front_inputs(
    request: &OptimizationRequest,
    min_eval_examples: usize,
) -> Result<(), FrontRefusal> {
    if request.optimizer != crate::OptimizerKind::Gepa || request.candidate_evaluations.is_empty() {
        return Ok(());
    }
    let test_examples = request
        .corpus
        .examples
        .iter()
        .filter(|example| example.split == ExampleSplit::Test)
        .count();
    if test_examples < min_eval_examples {
        return Err(FrontRefusal::EvalTooSmall {
            test_examples,
            minimum: min_eval_examples,
        });
    }
    if let Some(evaluation) = request
        .candidate_evaluations
        .iter()
        .find(|evaluation| evaluation.benchmark_score.is_none())
    {
        return Err(FrontRefusal::ScoreLength {
            subject_ref: evaluation.subject_ref.as_str().to_string(),
        });
    }
    Ok(())
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
    let test_refs: Vec<&str> = tests
        .iter()
        .map(|example| example.example_ref.as_str())
        .collect();
    let evaluation_set_digest = digest_text(HELD_OUT_SET_DIGEST_DOMAIN, &test_refs);
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
        evaluation_set_digest,
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
