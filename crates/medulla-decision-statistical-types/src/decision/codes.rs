//! Shared request refusal codes of the decision plane (MEDULLA-DECISION-ENGINE-R010.100.1).
//!
//! A code here may be returned by any family. A family's own `REFUSALS` table
//! must not repeat one of these, nor another family's code, so one code names
//! one refusal across the whole decision surface.

use medulla_types::refusal_code::RefusalCode;

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
/// Policy-evolution refusal: a held-out trajectory is one the evaluated version trained on.
pub const POLICY_EVALUATION_OVERLAPS_TRAINING: RefusalCode =
    RefusalCode::new("POLICY_EVALUATION_OVERLAPS_TRAINING");
/// Policy-evolution refusal: a held-out trajectory is one the compared champion trained on
/// (MEDULLA-DECISION-ENGINE-R811.4).
pub const POLICY_SPLIT_OVERLAP: RefusalCode = RefusalCode::new("POLICY_SPLIT_OVERLAP");
/// Policy-evolution refusal: an evaluation of a decision head or program subject
/// (MEDULLA-DECISION-ENGINE-R811.4).
pub const POLICY_SUBJECT_UNSUPPORTED: RefusalCode = RefusalCode::new("POLICY_SUBJECT_UNSUPPORTED");
/// Policy-evolution refusal native authority required.
pub const POLICY_NATIVE_AUTHORITY_REQUIRED: RefusalCode =
    RefusalCode::new("POLICY_NATIVE_AUTHORITY_REQUIRED");

/// A typed edge whose producer output schema is not the consumer input schema.
pub const PRIMITIVE_EDGE_MISMATCH: RefusalCode = RefusalCode::new("PRIMITIVE_EDGE_MISMATCH");
/// A replacement of another primitive kind.
pub const PRIMITIVE_KIND_MISMATCH: RefusalCode = RefusalCode::new("PRIMITIVE_KIND_MISMATCH");
/// A replacement that changes the input or output schema digest.
pub const PRIMITIVE_SCHEMA_CHANGED: RefusalCode = RefusalCode::new("PRIMITIVE_SCHEMA_CHANGED");
/// A replacement that drops a capability the old primitive had.
pub const PRIMITIVE_CAPABILITY_LOST: RefusalCode = RefusalCode::new("PRIMITIVE_CAPABILITY_LOST");
/// A replacement with an effect the old primitive did not have.
pub const PRIMITIVE_EFFECT_WIDENED: RefusalCode = RefusalCode::new("PRIMITIVE_EFFECT_WIDENED");
/// A primitive graph with more edges than the primitive.max_edges knob admits.
pub const PRIMITIVE_GRAPH_TOO_LARGE: RefusalCode = RefusalCode::new("PRIMITIVE_GRAPH_TOO_LARGE");

/// A coding agent offered without a discovery receipt.
pub const DISCOVERY_RECEIPT_MISSING: RefusalCode = RefusalCode::new("DISCOVERY_RECEIPT_MISSING");
/// A discovery receipt older than the coding_agent.max_receipt_age knob.
pub const DISCOVERY_RECEIPT_STALE: RefusalCode = RefusalCode::new("DISCOVERY_RECEIPT_STALE");
/// Adjacent skill transformations whose output and input schemas differ.
pub const TRANSFORMATION_SCHEMA_MISMATCH: RefusalCode =
    RefusalCode::new("TRANSFORMATION_SCHEMA_MISMATCH");
/// The last skill does not produce the artifact contract the plan binds.
pub const ARTIFACT_DIGEST_CHANGED: RefusalCode = RefusalCode::new("ARTIFACT_DIGEST_CHANGED");

/// An edit concentration question whose policy node is missing.
pub const NO_CONCENTRATION_POLICY: RefusalCode = RefusalCode::new("NO_CONCENTRATION_POLICY");
/// An edit concentration question whose edits were judged under different policy revisions.
pub const MIXED_POLICY_REVISION: RefusalCode = RefusalCode::new("MIXED_POLICY_REVISION");
/// The `edit_concentration` family's refusal table (MEDULLA-DECISION-ENGINE-R173.3).
pub const CONCENTRATION_REFUSALS: &[RefusalCode] =
    &[NO_CONCENTRATION_POLICY, MIXED_POLICY_REVISION];

/// A team coordination question whose channel reads available but whose signal is older than `team.signal_max_age`.
pub const TEAM_SIGNAL_STALE: RefusalCode = RefusalCode::new("TEAM_SIGNAL_STALE");
/// A team coordination question whose channel is unavailable and whose team has no explicitly bound member.
pub const TEAM_NO_FALLBACK_AVAILABLE: RefusalCode = RefusalCode::new("TEAM_NO_FALLBACK_AVAILABLE");
/// A team coordination question over a graph holding a visible library member with no explicit team binding.
pub const TEAM_MEMBER_UNBOUND: RefusalCode = RefusalCode::new("TEAM_MEMBER_UNBOUND");
/// A team coordination question that carries no typed channel signal.
pub const TEAM_SIGNAL_MISSING: RefusalCode = RefusalCode::new("TEAM_SIGNAL_MISSING");
/// The `team_coordination` family's refusal table (MEDULLA-DECISION-ENGINE-R170.4).
pub const TEAM_REFUSALS: &[RefusalCode] = &[
    TEAM_MEMBER_UNBOUND,
    TEAM_SIGNAL_MISSING,
    TEAM_SIGNAL_STALE,
    TEAM_NO_FALLBACK_AVAILABLE,
];

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
    POLICY_EVALUATION_OVERLAPS_TRAINING,
    POLICY_SPLIT_OVERLAP,
    POLICY_SUBJECT_UNSUPPORTED,
    POLICY_NATIVE_AUTHORITY_REQUIRED,
    PRIMITIVE_EDGE_MISMATCH,
    PRIMITIVE_KIND_MISMATCH,
    PRIMITIVE_SCHEMA_CHANGED,
    PRIMITIVE_CAPABILITY_LOST,
    PRIMITIVE_EFFECT_WIDENED,
    PRIMITIVE_GRAPH_TOO_LARGE,
    DISCOVERY_RECEIPT_MISSING,
    DISCOVERY_RECEIPT_STALE,
    TRANSFORMATION_SCHEMA_MISMATCH,
    ARTIFACT_DIGEST_CHANGED,
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
/// A held-out evaluation cannot qualify a challenger: no evaluation record
/// exists for it, or its safety outcome failed (MEDULLA-DECISION-ENGINE-R811.8).
pub const PROMOTION_UNEVALUATED: RefusalCode = RefusalCode::new("PROMOTION_UNEVALUATED");
/// A champion move names a champion that is no longer the active one
/// (MEDULLA-DECISION-ENGINE-R805.4).
pub const PROMOTION_CAS_CONFLICT: RefusalCode = RefusalCode::new("PROMOTION_CAS_CONFLICT");
/// The record a champion move cites is not a `promote` verdict for the target
/// (MEDULLA-DECISION-ENGINE-R805.4).
pub const PROMOTION_RECEIPT_MISMATCH: RefusalCode = RefusalCode::new("PROMOTION_RECEIPT_MISMATCH");
/// A champion rollback has no earlier target to return to
/// (MEDULLA-DECISION-ENGINE-R805.4).
pub const ROLLBACK_NO_PARENT: RefusalCode = RefusalCode::new("ROLLBACK_NO_PARENT");
/// A champion pointer read names a key that has no active revision
/// (MEDULLA-DECISION-ENGINE-R805.13).
pub const POINTER_NOT_FOUND: RefusalCode = RefusalCode::new("POINTER_NOT_FOUND");
/// A review names a revision no row of the tenant holds
/// (MEDULLA-DECISION-ENGINE-R805.15).
pub const REVISION_NOT_FOUND: RefusalCode = RefusalCode::new("REVISION_NOT_FOUND");
/// A review's digest differs from the digest of the revision it names
/// (MEDULLA-DECISION-ENGINE-R805.15).
pub const REVISION_DIGEST_MISMATCH: RefusalCode = RefusalCode::new("REVISION_DIGEST_MISMATCH");
/// The reviewer of a revision is its author (MEDULLA-DECISION-ENGINE-R805.15).
pub const REVIEW_SELF_APPROVAL: RefusalCode = RefusalCode::new("REVIEW_SELF_APPROVAL");
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

/// A retrieval strategy did not beat its champion by `retrieval.adopt_margin`
/// on the paired held-out run (MEDULLA-DECISION-ENGINE-R821).
pub const ADOPTION_NOT_PROVEN: RefusalCode = RefusalCode::new("ADOPTION_NOT_PROVEN");

/// A caller asked to promote on a delta inside the noise band (MEDULLA-DECISION-ENGINE-R712.4).
pub const ABLATION_WITHIN_NOISE: RefusalCode = RefusalCode::new("ABLATION_WITHIN_NOISE");
/// Fewer identical runs than `eval.ablation.identical_runs`, or none recorded for the skill.
pub const ABLATION_NOISE_BAND_MISSING: RefusalCode =
    RefusalCode::new("ABLATION_NOISE_BAND_MISSING");
/// A canary traffic share outside the bounds of `canary.traffic_ppm`.
pub const CANARY_TRAFFIC_OUT_OF_BOUNDS: RefusalCode =
    RefusalCode::new("CANARY_TRAFFIC_OUT_OF_BOUNDS");
/// A canary needs a champion revision and the skill has none.
pub const CANARY_NO_CHAMPION: RefusalCode = RefusalCode::new("CANARY_NO_CHAMPION");
/// A compiled program names a model the model catalog lacks.
pub const COMPILE_MODEL_UNKNOWN: RefusalCode = RefusalCode::new("COMPILE_MODEL_UNKNOWN");

/// Every promotion and calibration code, in declaration order.
pub const PROMOTION: &[RefusalCode] = &[
    PROMOTION_UNEVALUATED,
    PROMOTION_CAS_CONFLICT,
    PROMOTION_RECEIPT_MISMATCH,
    ROLLBACK_NO_PARENT,
    POINTER_NOT_FOUND,
    REVISION_NOT_FOUND,
    REVISION_DIGEST_MISMATCH,
    REVIEW_SELF_APPROVAL,
    EVAL_SET_NOT_INDEPENDENT,
    PROMOTION_EVAL_TOO_SMALL,
    PROMOTION_NO_BASELINE,
    PROMOTION_JUDGE_ONLY,
    MEASUREMENT_NOT_NEWER,
    REVIEW_APPROVAL_REQUIRED,
    REVIEW_NOT_INDEPENDENT,
    CALIBRATION_TOO_FEW_PAIRS,
    ADOPTION_NOT_PROVEN,
    ABLATION_WITHIN_NOISE,
    ABLATION_NOISE_BAND_MISSING,
    CANARY_TRAFFIC_OUT_OF_BOUNDS,
    CANARY_NO_CHAMPION,
    COMPILE_MODEL_UNKNOWN,
];

// The `rank` family's refusals (MEDULLA-DECISION-ENGINE-R130.4).
/// A ranked option has no calibrated probability; no likelihood substitutes.
pub const UNCALIBRATED_CANDIDATE: RefusalCode = RefusalCode::new("UNCALIBRATED_CANDIDATE");

/// A `catalog.` question id names no published decision point
/// (MEDULLA-DECISION-ENGINE-R829.1); the client answers its own fallback.
pub const QUESTION_UNPUBLISHED: RefusalCode = RefusalCode::new("QUESTION_UNPUBLISHED");

/// Every `rank` family code, in declaration order.
pub const RANK: &[RefusalCode] = &[UNCALIBRATED_CANDIDATE, QUESTION_UNPUBLISHED];

// The `resolve_entity` family's refusals (MEDULLA-DECISION-ENGINE-R032.2.2).
/// A pair option names no left or right IRI, or one that is not an IRI.
pub const ENTITY_PAIR_INVALID: RefusalCode = RefusalCode::new("ENTITY_PAIR_INVALID");
/// A pair option names the same entity on both sides.
pub const ENTITY_PAIR_SELF: RefusalCode = RefusalCode::new("ENTITY_PAIR_SELF");

/// An entity link approval names no approver (MEDULLA-UNIFIED-DATA-PLANE-R030.701);
/// the spelling is `ApprovalRefusal::ApproverRequired.code()`.
pub const LINK_APPROVER_MISSING: RefusalCode = RefusalCode::new("LINK_APPROVER_MISSING");

/// The graph named for candidate generation holds more embedded nodes than
/// `er.index_max_nodes` allows an index to be built over.
pub const ER_INDEX_TOO_LARGE: RefusalCode = RefusalCode::new("ER_INDEX_TOO_LARGE");

/// A proposal pairs a node the asking tenant holds with one it does not hold
/// (another tenant's node and an absent one read the same).
pub const ER_CROSS_TENANT: RefusalCode = RefusalCode::new("ER_CROSS_TENANT");

/// The top proposal scored under the head's certified act threshold.
pub const ER_BELOW_THRESHOLD: RefusalCode = RefusalCode::new("ER_BELOW_THRESHOLD");

/// A probabilistic proposal is held back while the accept rate has drifted.
pub const ER_DRIFT_HOLD: RefusalCode = RefusalCode::new("ER_DRIFT_HOLD");

/// Every `resolve_entity` family code, in declaration order.
pub const RESOLVE_ENTITY: &[RefusalCode] = &[
    ENTITY_PAIR_INVALID,
    ENTITY_PAIR_SELF,
    LINK_APPROVER_MISSING,
    ER_INDEX_TOO_LARGE,
    ER_CROSS_TENANT,
    ER_BELOW_THRESHOLD,
    ER_DRIFT_HOLD,
];

// The `round_verdict` and `stage_allocation` families' refusals
// (MEDULLA-DECISION-ENGINE-R136.5).
/// A round declared no fixes and no regressions; there is nothing to verify.
pub const ROUND_NO_PREDICTION: RefusalCode = RefusalCode::new("ROUND_NO_PREDICTION");
/// Running every stage on the base model already exceeds the run budget.
pub const ALLOCATION_BUDGET_BELOW_MINIMUM: RefusalCode =
    RefusalCode::new("ALLOCATION_BUDGET_BELOW_MINIMUM");
/// No offered model is adequate for the run.
pub const ALLOCATION_NO_ADEQUATE_MODEL: RefusalCode =
    RefusalCode::new("ALLOCATION_NO_ADEQUATE_MODEL");

// The `criterion_review` family's refusals (MEDULLA-DECISION-ENGINE-R136.101).
/// The prior and new reports were graded against different rubrics.
pub const CRITERION_RUBRIC_MISMATCH: RefusalCode = RefusalCode::new("CRITERION_RUBRIC_MISMATCH");
/// A criterion satisfied in the prior report is no longer satisfied.
pub const CRITERION_REGRESSION: RefusalCode = RefusalCode::new("CRITERION_REGRESSION");

/// A criterion of the rubric has no state in the report
/// (MEDULLA-DECISION-ENGINE-R136.102).
pub const CRITERION_UNGRADED: RefusalCode = RefusalCode::new("CRITERION_UNGRADED");
/// The report grades another artifact than the one the request declares.
pub const CRITERION_ARTIFACT_MISMATCH: RefusalCode =
    RefusalCode::new("CRITERION_ARTIFACT_MISMATCH");
/// A criterion state carries no evidence reference.
pub const CRITERION_EVIDENCE_MISSING: RefusalCode = RefusalCode::new("CRITERION_EVIDENCE_MISSING");
/// The judge's score trend runs away from the independent outcome trend
/// (MEDULLA-DECISION-ENGINE-R136.103).
pub const JUDGE_OUTCOME_DIVERGENCE: RefusalCode = RefusalCode::new("JUDGE_OUTCOME_DIVERGENCE");

/// Every `criterion_review` family code, in declaration order.
pub const CRITERION_REVIEW: &[RefusalCode] = &[
    CRITERION_RUBRIC_MISMATCH,
    CRITERION_REGRESSION,
    CRITERION_UNGRADED,
    CRITERION_ARTIFACT_MISMATCH,
    CRITERION_EVIDENCE_MISSING,
    JUDGE_OUTCOME_DIVERGENCE,
];

// The `generated_code_admission` family's refusals (MEDULLA-DECISION-ENGINE-R138.100.2).
/// The verifier receipt is absent, or holds fewer cases than the policy requires.
pub const VERIFIER_RECEIPT_MISSING: RefusalCode = RefusalCode::new("VERIFIER_RECEIPT_MISSING");
/// An attack case is neither retested nor failed: open or fixed only.
pub const VERIFIER_CASE_UNRESOLVED: RefusalCode = RefusalCode::new("VERIFIER_CASE_UNRESOLVED");
/// The receipt, or a retest, names another artifact than the proposed one.
pub const VERIFIER_ARTIFACT_STALE: RefusalCode = RefusalCode::new("VERIFIER_ARTIFACT_STALE");
/// The receipt was produced against another test set than the declared one.
pub const VERIFIER_TESTS_CHANGED: RefusalCode = RefusalCode::new("VERIFIER_TESTS_CHANGED");
/// A retest of an attack case failed.
pub const VERIFIER_RECHECK_FAILED: RefusalCode = RefusalCode::new("VERIFIER_RECHECK_FAILED");
/// The attack policy or the verifier is not the current revision.
pub const VERIFIER_REVISION_STALE: RefusalCode = RefusalCode::new("VERIFIER_REVISION_STALE");

/// Every `generated_code_admission` family code, in declaration order.
pub const GENERATED_CODE_ADMISSION: &[RefusalCode] = &[
    VERIFIER_RECEIPT_MISSING,
    VERIFIER_CASE_UNRESOLVED,
    VERIFIER_ARTIFACT_STALE,
    VERIFIER_TESTS_CHANGED,
    VERIFIER_RECHECK_FAILED,
    VERIFIER_REVISION_STALE,
];

// The `credential_reuse` family's refusals (MEDULLA-DECISION-ENGINE-R832).
/// The credential binding is owned by another principal than the question names.
pub const CREDENTIAL_OWNER_MISMATCH: RefusalCode = RefusalCode::new("CREDENTIAL_OWNER_MISMATCH");
/// The agent is not among the agents the binding names.
pub const CREDENTIAL_AGENT_NOT_BOUND: RefusalCode = RefusalCode::new("CREDENTIAL_AGENT_NOT_BOUND");
/// The binding was revoked.
pub const CREDENTIAL_REVOKED: RefusalCode = RefusalCode::new("CREDENTIAL_REVOKED");
/// The binding ended at or before the engine clock.
pub const CREDENTIAL_EXPIRED: RefusalCode = RefusalCode::new("CREDENTIAL_EXPIRED");
/// The connector or provider is not one the binding names.
pub const CREDENTIAL_CONNECTOR_NOT_BOUND: RefusalCode =
    RefusalCode::new("CREDENTIAL_CONNECTOR_NOT_BOUND");
/// A requested scope is outside the binding's scopes.
pub const CREDENTIAL_SCOPE_EXCEEDED: RefusalCode = RefusalCode::new("CREDENTIAL_SCOPE_EXCEEDED");
/// The purpose is not one the binding allows.
pub const CREDENTIAL_PURPOSE_NOT_ALLOWED: RefusalCode =
    RefusalCode::new("CREDENTIAL_PURPOSE_NOT_ALLOWED");
/// No binding accompanies the question, so there is nothing to decide on.
pub const CREDENTIAL_BINDING_MISSING: RefusalCode = RefusalCode::new("CREDENTIAL_BINDING_MISSING");

/// Every `credential_reuse` family code, in declaration order.
pub const CREDENTIAL_REUSE: &[RefusalCode] = &[
    CREDENTIAL_OWNER_MISMATCH,
    CREDENTIAL_AGENT_NOT_BOUND,
    CREDENTIAL_REVOKED,
    CREDENTIAL_EXPIRED,
    CREDENTIAL_CONNECTOR_NOT_BOUND,
    CREDENTIAL_SCOPE_EXCEEDED,
    CREDENTIAL_PURPOSE_NOT_ALLOWED,
    CREDENTIAL_BINDING_MISSING,
];

// The `probe_verdict` family's refusals (MEDULLA-DECISION-ENGINE-R138.4).
/// No scorer of the tenant matches the attack's category.
pub const NO_SCORER_FOR_CATEGORY: RefusalCode = RefusalCode::new("NO_SCORER_FOR_CATEGORY");
/// `attack_id` names no visible attack.
pub const PROBE_ATTACK_UNKNOWN: RefusalCode = RefusalCode::new("PROBE_ATTACK_UNKNOWN");
/// A probe result names a scorer that is not selected.
pub const PROBE_RESULT_SCORER_UNKNOWN: RefusalCode =
    RefusalCode::new("PROBE_RESULT_SCORER_UNKNOWN");
/// A probe score is above 1000 per mille.
pub const PROBE_SCORE_OUT_OF_RANGE: RefusalCode = RefusalCode::new("PROBE_SCORE_OUT_OF_RANGE");

/// Every `probe_verdict` family code, in declaration order.
pub const PROBE_VERDICT: &[RefusalCode] = &[
    NO_SCORER_FOR_CATEGORY,
    PROBE_ATTACK_UNKNOWN,
    PROBE_RESULT_SCORER_UNKNOWN,
    PROBE_SCORE_OUT_OF_RANGE,
];

// The `probe_selection` family's refusal (MEDULLA-DECISION-ENGINE-R818).
/// A goal selects no legal attack of the tenant's catalog.
pub const NO_CATALOG_ENTRY_SELECTED: RefusalCode = RefusalCode::new("NO_CATALOG_ENTRY_SELECTED");

/// Every `probe_selection` family code, in declaration order.
pub const PROBE_SELECTION: &[RefusalCode] = &[NO_CATALOG_ENTRY_SELECTED];

/// A caller-preferred route option is not legal: the ladder did not score it,
/// or its score lies outside the tie epsilon of the best. Recorded as the
/// subject of the `preference_not_honoured:<option>` premise
/// (MEDULLA-DECISION-ENGINE-R039.3); the question still answers.
pub const PREFERENCE_NOT_LEGAL: RefusalCode = RefusalCode::new("PREFERENCE_NOT_LEGAL");

/// Every `route` family code, in declaration order.
pub const ROUTE: &[RefusalCode] = &[PREFERENCE_NOT_LEGAL];
/// Every `round_verdict` family code, in declaration order.
pub const ROUND_VERDICT: &[RefusalCode] = &[ROUND_NO_PREDICTION];

/// Every `stage_allocation` family code, in declaration order.
pub const STAGE_ALLOCATION: &[RefusalCode] = &[
    ALLOCATION_BUDGET_BELOW_MINIMUM,
    ALLOCATION_NO_ADEQUATE_MODEL,
];

// Recursion-mode signal refusals (MEDULLA-DECISION-ENGINE-R137.13). Each is a
// request refusal whose wire error is `PARAMETER_INVALID`.
/// The signal digest does not match the signal fields.
pub const SIGNAL_DIGEST_MISMATCH: RefusalCode = RefusalCode::new("SIGNAL_DIGEST_MISMATCH");
/// The signal was observed longer ago than `context.signal_max_age_ms`.
pub const SIGNAL_STALE: RefusalCode = RefusalCode::new("SIGNAL_STALE");
/// A signal reference does not resolve in the caller's tenant.
pub const SIGNAL_CROSS_TENANT: RefusalCode = RefusalCode::new("SIGNAL_CROSS_TENANT");
/// A signal value is missing, negative or not a number.
pub const SIGNAL_INVALID: RefusalCode = RefusalCode::new("SIGNAL_INVALID");

/// Every recursion-mode signal code, in declaration order.
pub const RECURSION_SIGNALS: &[RefusalCode] = &[
    SIGNAL_DIGEST_MISMATCH,
    SIGNAL_STALE,
    SIGNAL_CROSS_TENANT,
    SIGNAL_INVALID,
];

// The `connector_write_back` family's refusals (MEDULLA-DECISION-ENGINE-R615.4).
/// A release of a queue holding a proposal that has no approval.
pub const PROPOSAL_UNAPPROVED: RefusalCode = RefusalCode::new("PROPOSAL_UNAPPROVED");
/// Two sources disagree on the value a write-back proposal would change.
pub const WRITEBACK_SOURCE_CONFLICT: RefusalCode = RefusalCode::new("WRITEBACK_SOURCE_CONFLICT");

/// Every `connector_write_back` family code, in declaration order.
pub const CONNECTOR_WRITE_BACK: &[RefusalCode] = &[PROPOSAL_UNAPPROVED, WRITEBACK_SOURCE_CONFLICT];
