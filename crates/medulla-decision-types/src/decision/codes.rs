//! Shared request refusal codes of the decision plane (MEDULLA-DECISION-ENGINE-R010.100.1).
//!
//! A code here may be returned by any family. A family's own `REFUSALS` table
//! must not repeat one of these, nor another family's code, so one code names
//! one refusal across the whole decision surface.

use super::record::outcome::RefusalCode;
use super::statistical::QuestionKind;

/// The caller may not read or act on what the request names.
pub const ACCESS_DENIED: RefusalCode = RefusalCode::new("ACCESS_DENIED");
/// A request parameter is missing, malformed or out of range.
pub const PARAMETER_INVALID: RefusalCode = RefusalCode::new("PARAMETER_INVALID");
/// A request reused an idempotency key with a different body.
pub const IDEMPOTENCY_CONFLICT: RefusalCode = RefusalCode::new("IDEMPOTENCY_CONFLICT");

/// Every shared request code, in declaration order.
pub const SHARED: &[RefusalCode] = &[ACCESS_DENIED, PARAMETER_INVALID, IDEMPOTENCY_CONFLICT];

/// Codes folded in from the per-feature refusal enums
/// (MEDULLA-DECISION-ENGINE-R010.100.2). Each is also a registered wire code.
/// A head-kind name outside the closed statistical set.
pub const HEAD_KIND_NOT_STATISTICAL: RefusalCode = RefusalCode::new("HEAD_KIND_NOT_STATISTICAL");
/// No candidate subset covers every required capability.
pub const TOOL_SUBSET_CAPABILITY_UNCOVERABLE: RefusalCode =
    RefusalCode::new("TOOL_SUBSET_CAPABILITY_UNCOVERABLE");
/// Every covering subset exceeds the context budget.
pub const TOOL_SUBSET_BUDGET_EXCEEDED: RefusalCode =
    RefusalCode::new("TOOL_SUBSET_BUDGET_EXCEEDED");
/// A certificate produced against a different model.
pub const CERTIFICATE_MODEL_DIGEST_MISMATCH: RefusalCode =
    RefusalCode::new("CERTIFICATE_MODEL_DIGEST_MISMATCH");
/// A certificate that claims more search than the solver budget.
pub const CERTIFICATE_BRANCH_BUDGET_EXCEEDED: RefusalCode =
    RefusalCode::new("CERTIFICATE_BRANCH_BUDGET_EXCEEDED");
/// A ranking request that names a verifier.
pub const ANSWER_RANKING_VERIFIER_NAMED: RefusalCode =
    RefusalCode::new("ANSWER_RANKING_VERIFIER_NAMED");
/// A ranking request under two candidates.
pub const ANSWER_RANKING_TOO_FEW_CANDIDATES: RefusalCode =
    RefusalCode::new("ANSWER_RANKING_TOO_FEW_CANDIDATES");
/// A ranking that needs more revisions than allowed.
pub const ANSWER_RANKING_REVISION_BOUND_EXCEEDED: RefusalCode =
    RefusalCode::new("ANSWER_RANKING_REVISION_BOUND_EXCEEDED");
/// A retain count of zero or above the candidate count.
pub const ANSWER_RANKING_INVALID_RETAIN: RefusalCode =
    RefusalCode::new("ANSWER_RANKING_INVALID_RETAIN");
/// A candidate named twice.
pub const ANSWER_RANKING_DUPLICATE_CANDIDATE: RefusalCode =
    RefusalCode::new("ANSWER_RANKING_DUPLICATE_CANDIDATE");
/// A comparison naming a candidate outside the request.
pub const ANSWER_RANKING_UNKNOWN_CANDIDATE: RefusalCode =
    RefusalCode::new("ANSWER_RANKING_UNKNOWN_CANDIDATE");
/// A pair compared twice.
pub const ANSWER_RANKING_DUPLICATE_COMPARISON: RefusalCode =
    RefusalCode::new("ANSWER_RANKING_DUPLICATE_COMPARISON");
/// Pairs left uncompared.
pub const ANSWER_RANKING_INCOMPLETE_COMPARISONS: RefusalCode =
    RefusalCode::new("ANSWER_RANKING_INCOMPLETE_COMPARISONS");
/// A split plan over a budget limit.
pub const CONTEXT_BUDGET_EXHAUSTED: RefusalCode = RefusalCode::new("CONTEXT_BUDGET_EXHAUSTED");
/// A split plan over the item bound.
pub const CONTEXT_BUDGET_TOO_MANY_ITEMS: RefusalCode =
    RefusalCode::new("CONTEXT_BUDGET_TOO_MANY_ITEMS");
/// A triage with no symbol matching the symptom.
pub const TRIAGE_NO_SYMBOL_MATCH: RefusalCode = RefusalCode::new("TRIAGE_NO_SYMBOL_MATCH");
/// A triage with no owner for the suspects.
pub const TRIAGE_NO_OWNER: RefusalCode = RefusalCode::new("TRIAGE_NO_OWNER");
/// A triage without enough evidence.
pub const TRIAGE_INSUFFICIENT_EVIDENCE: RefusalCode =
    RefusalCode::new("TRIAGE_INSUFFICIENT_EVIDENCE");
/// An option declaring more capabilities than the encoding bound.
pub const ENCODE_TOO_MANY_CAPABILITIES: RefusalCode =
    RefusalCode::new("ENCODE_TOO_MANY_CAPABILITIES");
/// A contrast candidate set under two.
pub const HARNESS_CONTRAST_MIN_CANDIDATES: RefusalCode =
    RefusalCode::new("HARNESS_CONTRAST_MIN_CANDIDATES");
/// A contrast candidate count over eight.
pub const HARNESS_CONTRAST_TOO_MANY_CANDIDATES: RefusalCode =
    RefusalCode::new("HARNESS_CONTRAST_TOO_MANY_CANDIDATES");
/// A candidate version the graph does not hold.
pub const HARNESS_COMPONENT_VERSION_UNKNOWN: RefusalCode =
    RefusalCode::new("HARNESS_COMPONENT_VERSION_UNKNOWN");
/// A probe trace that disagrees with the stored runs.
pub const HARNESS_CONTRAST_TRACE_MISMATCH: RefusalCode =
    RefusalCode::new("HARNESS_CONTRAST_TRACE_MISMATCH");
/// No candidate improves on the current harness.
pub const HARNESS_CONTRAST_NO_IMPROVEMENT: RefusalCode =
    RefusalCode::new("HARNESS_CONTRAST_NO_IMPROVEMENT");
/// A candidate that was never probed.
pub const HARNESS_CONTRAST_INSUFFICIENT_EVIDENCE: RefusalCode =
    RefusalCode::new("HARNESS_CONTRAST_INSUFFICIENT_EVIDENCE");
/// Policy-evolution refusal invalid record.
pub const POLICY_INVALID_RECORD: RefusalCode = RefusalCode::new("POLICY_INVALID_RECORD");
/// Policy-evolution refusal capability missing.
pub const POLICY_CAPABILITY_MISSING: RefusalCode = RefusalCode::new("POLICY_CAPABILITY_MISSING");
/// Policy-evolution refusal capture disabled.
pub const POLICY_CAPTURE_DISABLED: RefusalCode = RefusalCode::new("POLICY_CAPTURE_DISABLED");
/// Policy-evolution refusal train disabled.
pub const POLICY_TRAIN_DISABLED: RefusalCode = RefusalCode::new("POLICY_TRAIN_DISABLED");
/// Policy-evolution refusal logprobs unsupported.
pub const POLICY_LOGPROBS_UNSUPPORTED: RefusalCode =
    RefusalCode::new("POLICY_LOGPROBS_UNSUPPORTED");
/// Policy-evolution refusal sampler mismatch.
pub const POLICY_SAMPLER_MISMATCH: RefusalCode = RefusalCode::new("POLICY_SAMPLER_MISMATCH");
/// Policy-evolution refusal trajectory missing.
pub const POLICY_TRAJECTORY_MISSING: RefusalCode = RefusalCode::new("POLICY_TRAJECTORY_MISSING");
/// Policy-evolution refusal trajectory length mismatch.
pub const POLICY_TRAJECTORY_LENGTH_MISMATCH: RefusalCode =
    RefusalCode::new("POLICY_TRAJECTORY_LENGTH_MISMATCH");
/// Policy-evolution refusal blob missing.
pub const POLICY_BLOB_MISSING: RefusalCode = RefusalCode::new("POLICY_BLOB_MISSING");
/// Policy-evolution refusal record missing.
pub const POLICY_RECORD_MISSING: RefusalCode = RefusalCode::new("POLICY_RECORD_MISSING");
/// Policy-evolution refusal run not succeeded.
pub const POLICY_RUN_NOT_SUCCEEDED: RefusalCode = RefusalCode::new("POLICY_RUN_NOT_SUCCEEDED");
/// Policy-evolution refusal capture ineligible.
pub const POLICY_CAPTURE_INELIGIBLE: RefusalCode = RefusalCode::new("POLICY_CAPTURE_INELIGIBLE");
/// Policy-evolution refusal record tampered.
pub const POLICY_RECORD_TAMPERED: RefusalCode = RefusalCode::new("POLICY_RECORD_TAMPERED");
/// Policy-evolution refusal native authority required.
pub const POLICY_NATIVE_AUTHORITY_REQUIRED: RefusalCode =
    RefusalCode::new("POLICY_NATIVE_AUTHORITY_REQUIRED");

/// An edit concentration question whose policy node is missing.
pub const NO_CONCENTRATION_POLICY: RefusalCode = RefusalCode::new("NO_CONCENTRATION_POLICY");
/// An edit concentration question whose edits were judged under different policy revisions.
pub const MIXED_POLICY_REVISION: RefusalCode = RefusalCode::new("MIXED_POLICY_REVISION");
/// The `edit_concentration` family's refusal table (MEDULLA-DECISION-ENGINE-R173.3).
pub const CONCENTRATION_REFUSALS: &[RefusalCode] = &[NO_CONCENTRATION_POLICY, MIXED_POLICY_REVISION];

/// Every folded code, in declaration order.
pub const FOLDED: &[RefusalCode] = &[
    HEAD_KIND_NOT_STATISTICAL,
    TOOL_SUBSET_CAPABILITY_UNCOVERABLE,
    TOOL_SUBSET_BUDGET_EXCEEDED,
    CERTIFICATE_MODEL_DIGEST_MISMATCH,
    CERTIFICATE_BRANCH_BUDGET_EXCEEDED,
    ANSWER_RANKING_VERIFIER_NAMED,
    ANSWER_RANKING_TOO_FEW_CANDIDATES,
    ANSWER_RANKING_REVISION_BOUND_EXCEEDED,
    ANSWER_RANKING_INVALID_RETAIN,
    ANSWER_RANKING_DUPLICATE_CANDIDATE,
    ANSWER_RANKING_UNKNOWN_CANDIDATE,
    ANSWER_RANKING_DUPLICATE_COMPARISON,
    ANSWER_RANKING_INCOMPLETE_COMPARISONS,
    CONTEXT_BUDGET_EXHAUSTED,
    CONTEXT_BUDGET_TOO_MANY_ITEMS,
    TRIAGE_NO_SYMBOL_MATCH,
    TRIAGE_NO_OWNER,
    TRIAGE_INSUFFICIENT_EVIDENCE,
    ENCODE_TOO_MANY_CAPABILITIES,
    HARNESS_CONTRAST_MIN_CANDIDATES,
    HARNESS_CONTRAST_TOO_MANY_CANDIDATES,
    HARNESS_COMPONENT_VERSION_UNKNOWN,
    HARNESS_CONTRAST_TRACE_MISMATCH,
    HARNESS_CONTRAST_NO_IMPROVEMENT,
    HARNESS_CONTRAST_INSUFFICIENT_EVIDENCE,
    POLICY_INVALID_RECORD,
    POLICY_CAPABILITY_MISSING,
    POLICY_CAPTURE_DISABLED,
    POLICY_TRAIN_DISABLED,
    POLICY_LOGPROBS_UNSUPPORTED,
    POLICY_SAMPLER_MISMATCH,
    POLICY_TRAJECTORY_MISSING,
    POLICY_TRAJECTORY_LENGTH_MISMATCH,
    POLICY_BLOB_MISSING,
    POLICY_RECORD_MISSING,
    POLICY_RUN_NOT_SUCCEEDED,
    POLICY_CAPTURE_INELIGIBLE,
    POLICY_RECORD_TAMPERED,
    POLICY_NATIVE_AUTHORITY_REQUIRED,
];

// Promotion and calibration refusals (MEDULLA-DECISION-ENGINE-R812.2, R805.11,
// R822). `medulla-decision-outcome-types` re-exports the table as
// `promotion::REFUSALS`.
/// The evaluation set was not independent of the training data.
pub const EVAL_SET_NOT_INDEPENDENT: RefusalCode = RefusalCode::new("EVAL_SET_NOT_INDEPENDENT");
/// The held-out evaluation has fewer cases than the knob requires.
pub const PROMOTION_EVAL_TOO_SMALL: RefusalCode = RefusalCode::new("PROMOTION_EVAL_TOO_SMALL");
/// The evidence names no champion score to compare with.
pub const PROMOTION_NO_BASELINE: RefusalCode = RefusalCode::new("PROMOTION_NO_BASELINE");
/// The only score is a model judge's; a judge never decides promotion.
pub const PROMOTION_JUDGE_ONLY: RefusalCode = RefusalCode::new("PROMOTION_JUDGE_ONLY");
/// A measured fact whose run is not newer than the champion's.
pub const MEASUREMENT_NOT_NEWER: RefusalCode = RefusalCode::new("MEASUREMENT_NOT_NEWER");
/// A governed set has no review approval.
pub const REVIEW_APPROVAL_REQUIRED: RefusalCode = RefusalCode::new("REVIEW_APPROVAL_REQUIRED");
/// The reviewer of a governed set is its author.
pub const REVIEW_NOT_INDEPENDENT: RefusalCode = RefusalCode::new("REVIEW_NOT_INDEPENDENT");
/// A calibration fit has fewer judged pairs than the knob requires.
pub const CALIBRATION_TOO_FEW_PAIRS: RefusalCode = RefusalCode::new("CALIBRATION_TOO_FEW_PAIRS");

/// Every promotion and calibration code, in declaration order.
pub const PROMOTION: &[RefusalCode] = &[
    EVAL_SET_NOT_INDEPENDENT,
    PROMOTION_EVAL_TOO_SMALL,
    PROMOTION_NO_BASELINE,
    PROMOTION_JUDGE_ONLY,
    MEASUREMENT_NOT_NEWER,
    REVIEW_APPROVAL_REQUIRED,
    REVIEW_NOT_INDEPENDENT,
    CALIBRATION_TOO_FEW_PAIRS,
];

/// The `REFUSALS` table of every registered family, keyed by its question
/// kind. A family module joins this list in the change that declares its table.
pub const FAMILY_TABLES: &[(QuestionKind, &[RefusalCode])] = &[
    (
        QuestionKind::RetrievalPlan,
        super::statistical::run_credit::REFUSALS,
    ),
    (QuestionKind::ThoughtSearch, super::thought_search::REFUSALS),
    (
        QuestionKind::TaskPlan,
        super::task_plan::trajectory_budget::REFUSALS,
    ),
    (
        QuestionKind::TaskPlan,
        super::task_plan::trajectory_projection::REFUSALS,
    ),
    (
        QuestionKind::TaskPlan,
        super::task_plan::branch_breadth::REFUSALS,
    ),
    (QuestionKind::TaskPlan, super::task_plan::pairwise::REFUSALS),
    (QuestionKind::ProgramLookup, super::program_store::REFUSALS),
    (QuestionKind::EditConcentration, CONCENTRATION_REFUSALS),
];

/// The `REFUSALS` table of every module that declares one in a crate below the
/// decision plane, keyed by module name. A module joins this list in the change
/// that declares its table; the uniqueness test collects it with every family
/// table (MEDULLA-DECISION-ENGINE-R010.100.1).
pub fn module_tables() -> Vec<(&'static str, Vec<RefusalCode>)> {
    use medulla_types::{
        belief_revision, claims, context_facts, meeting_transcript, participant, source_toc,
        transition_refused,
    };
    let codes = |pairs: &[(RefusalCode, &str)]| pairs.iter().map(|(code, _)| *code).collect();
    vec![
        ("belief_revision", codes(belief_revision::REFUSALS)),
        ("claims", codes(claims::REFUSALS)),
        ("context_facts", context_facts::REFUSALS.to_vec()),
        ("meeting_transcript", codes(meeting_transcript::REFUSALS)),
        ("participant", participant::REFUSALS.to_vec()),
        ("source_toc", source_toc::REFUSALS.to_vec()),
        ("transition_refused", transition_refused::REFUSALS.to_vec()),
        (
            "lifecycle",
            medulla_source_types::lifecycle::REFUSALS.to_vec(),
        ),
        (
            "topology_enforcement",
            medulla_agent_types::agent_topology::enforcement::REFUSALS.to_vec(),
        ),
    ]
}

/// Resolve a wire string to its registered code, shared codes first. The
/// assembly and commit surface's closed `DecisionErrorCode` set is registered
/// too: an assembly refusal folds under `RefusalOrigin::ASSEMBLE` with its own
/// code (MEDULLA-DECISION-ENGINE-R010.100.2).
pub fn resolve(code: &str) -> Option<RefusalCode> {
    SHARED
        .iter()
        .chain(FOLDED.iter())
        .chain(PROMOTION.iter())
        .chain(FAMILY_TABLES.iter().flat_map(|(_, table)| table.iter()))
        .chain(super::usage::REFUSALS.iter())
        .chain(super::importance_penalty::REFUSALS.iter())
        .chain(super::statistical::retrieval_generation::REFUSALS.iter())
        .copied()
        .chain(module_tables().into_iter().flat_map(|(_, table)| table))
        .find(|registered| registered.as_str() == code)
        .or_else(|| decision_error_code(code))
}

/// The registered code of one `DecisionErrorCode` token.
fn decision_error_code(code: &str) -> Option<RefusalCode> {
    super::DecisionErrorCode::ALL
        .iter()
        .find(|registered| registered.as_str() == code)
        .map(|registered| RefusalCode::new(registered.as_str()))
}
