//! `DecisionEval` `GroundedAnswerSubmit` (MEDULLA-DECISION-ENGINE-R100.101.3):
//! score grounded topic coverage and the soft-label unsupported-claim Brier
//! score, enforce scorer isolation and seal the receipt.
//!
//! The retrieved evidence comes from `UnifiedQuery`; `DecisionEval` only
//! scores and seals. A bound digest must equal the digest of the content it
//! pins, and a set revision pins one digest for good: the first accepted
//! evaluation stores `(set id, revision) -> digest` and a later one binding the
//! same revision with another digest, or an older revision than the newest
//! stored one, is refused. The scorer's recorded inputs must not include the
//! label set.

use medulla_decision_types::decision::knob_families::grounded::{
    coverage_knobs, score_digits, KNOBS,
};
use medulla_decision_types::decision::statistical::grounded_eval::{
    GroundedAnswerRequest, GroundedBinding, GroundedEvalReceipt, GroundedScope,
    GROUNDED_EVAL_SCHEMA_VERSION,
};
use medulla_decision_types::decision::statistical::ranking_eval::is_sha256_hex;
use medulla_decision_types::decision::DecisionJobOutput;
use medulla_persistence_records::agent_library::AgentLibraryStore;
use medulla_persistence_records::decision_jobs::{grounded_pin_key, grounded_pin_prefix};
use {
    medulla_decision_stats_records::decision::grounded_eval::topic_coverage,
    medulla_decision_stats_records::decision::grounded_eval::weighted_brier,
    medulla_decision_stats_records::decision::grounded_eval::GroundedRefusal,
    medulla_decision_stats_records::decision::grounded_eval::GROUNDED_DIGEST_MISMATCH,
    medulla_decision_stats_records::decision::grounded_eval::GROUNDED_REVISION_STALE,
    medulla_decision_stats_records::decision::grounded_eval::GROUNDED_SCORER_SAW_LABELS,
};

use super::stat_jobs::{ArtifactRows, JobRun};
use super::stat_support::resolve_policy;

/// The most stored revisions of one input set a scan reads.
const PIN_SCAN_LIMIT: usize = 4096;

fn refused(code: &str, detail: impl std::fmt::Display) -> String {
    format!("{code}: {detail}")
}

fn from_kernel(refusal: GroundedRefusal) -> String {
    refused(refusal.code, refusal.detail)
}

fn scope_name(scope: GroundedScope) -> &'static str {
    match scope {
        GroundedScope::SingleDocument => "single_document",
        GroundedScope::LocalMultiDocument => "local_multi_document",
        GroundedScope::GlobalCorpus => "global_corpus",
    }
}

/// Every bound digest is 64 lowercase hex characters and equals the digest of
/// the content it pins (the corpus, which the request does not carry, is
/// checked only against the store).
fn check_digests(request: &GroundedAnswerRequest) -> Result<(), String> {
    for (role, pinned) in request.bindings() {
        if !is_sha256_hex(&pinned.digest) {
            return Err(refused(
                GROUNDED_DIGEST_MISMATCH,
                format!("{role} digest is not 64 lowercase hex characters"),
            ));
        }
    }
    let content = [
        (
            "queries",
            &request.queries,
            request.queries_content_digest(),
        ),
        ("claims", &request.claims, request.claims_content_digest()),
        (
            "citations",
            &request.citations,
            request.citations_content_digest(),
        ),
        (
            "answers",
            &request.answers,
            request.answers_content_digest(),
        ),
        ("labels", &request.labels, request.labels_content_digest()),
    ];
    for (role, pinned, digest) in content {
        if pinned.digest != digest {
            return Err(refused(
                GROUNDED_DIGEST_MISMATCH,
                format!("{role} digest does not match the {role} content"),
            ));
        }
    }
    Ok(())
}

/// The scorer saw none of the labels, and every prediction names the recorded
/// scorer.
fn check_scorer(request: &GroundedAnswerRequest) -> Result<(), String> {
    let scorer = &request.scorer;
    if scorer
        .inputs_read
        .iter()
        .any(|input| input == &request.labels.digest)
    {
        return Err(refused(
            GROUNDED_SCORER_SAW_LABELS,
            format!("scorer={}@{}", scorer.scorer_id, scorer.scorer_revision),
        ));
    }
    if let Some(prediction) = request.predictions.iter().find(|p| {
        p.scorer_id != scorer.scorer_id
            || p.scorer_revision != scorer.scorer_revision
            || p.scorer_artifact_digest != scorer.scorer_artifact_digest
    }) {
        return Err(refused(
            GROUNDED_DIGEST_MISMATCH,
            format!(
                "claim={} was not scored by the recorded scorer",
                prediction.claim_id
            ),
        ));
    }
    Ok(())
}

/// Each bound set is the newest revision stored for its id, and a revision
/// already stored pins the digest it first bound.
fn check_pins(store: &AgentLibraryStore, request: &GroundedAnswerRequest) -> Result<(), String> {
    for (role, pinned) in request.bindings() {
        let prefix = grounded_pin_prefix(&pinned.id);
        let stored =
            store.decision_artifacts_with_prefix(&request.tenant_id, &prefix, PIN_SCAN_LIMIT)?;
        for (key, bytes) in stored {
            let suffix = &key[prefix.len()..];
            let Ok(revision) = suffix.parse::<u32>() else {
                continue;
            };
            if suffix.len() != 10 {
                continue;
            }
            if revision > pinned.revision {
                return Err(refused(
                    GROUNDED_REVISION_STALE,
                    format!(
                        "{role} {}@{} is older than stored revision {revision}",
                        pinned.id, pinned.revision
                    ),
                ));
            }
            if revision == pinned.revision && bytes != pinned.digest.as_bytes() {
                return Err(refused(
                    GROUNDED_DIGEST_MISMATCH,
                    format!(
                        "{role} {}@{} is stored with another digest",
                        pinned.id, pinned.revision
                    ),
                ));
            }
        }
    }
    Ok(())
}

/// Score `request` under its policy and seal the receipt; the rows pin every
/// bound revision's digest.
pub(super) fn run(store: &AgentLibraryStore, request: &GroundedAnswerRequest) -> JobRun {
    let policy = resolve_policy(store, &request.tenant_id, &request.policy)?;
    check_digests(request)?;
    check_scorer(request)?;
    check_pins(store, request)?;
    let knobs = coverage_knobs(&policy.policy);
    let coverage = topic_coverage(request.scope, &request.expected, &request.support, &knobs)
        .map_err(from_kernel)?;
    let digits = score_digits(&policy.policy);
    let brier = weighted_brier(
        &request.claim_ids,
        &request.predictions,
        &request.votes,
        digits,
    )
    .map_err(from_kernel)?;
    let mut premises = vec![
        format!("grounded:scope={}", scope_name(request.scope)),
        format!(
            "grounded:coverage={}/{}",
            coverage.covered, coverage.expected
        ),
        format!("grounded:brier={}/{}", brier.numerator, brier.denominator),
        format!(
            "grounded:scorer={}@{}",
            request.scorer.scorer_id, request.scorer.scorer_revision
        ),
    ];
    premises.extend(
        KNOBS
            .iter()
            .map(|spec| format!("knob:{}={}", spec.name, policy.policy.knob(spec).render())),
    );
    let receipt = GroundedEvalReceipt {
        schema_version: GROUNDED_EVAL_SCHEMA_VERSION,
        scope: request.scope,
        coverage,
        brier,
        evaluator_revision: request.evaluator_revision,
        policy_digest: policy.digest.clone(),
        bindings: request
            .bindings()
            .into_iter()
            .map(|(role, pinned)| GroundedBinding {
                role: role.to_string(),
                pinned: pinned.clone(),
            })
            .collect(),
        scorer_id: request.scorer.scorer_id.clone(),
        scorer_revision: request.scorer.scorer_revision,
        scorer_artifact_digest: request.scorer.scorer_artifact_digest.clone(),
        scorer_inputs: request.scorer.inputs_read.clone(),
        premises,
        receipt_digest: String::new(),
    }
    .sealed();
    let rows: ArtifactRows = request
        .bindings()
        .into_iter()
        .map(|(_, pinned)| {
            (
                grounded_pin_key(&pinned.id, pinned.revision),
                pinned.digest.clone().into_bytes(),
            )
        })
        .collect();
    Ok((
        DecisionJobOutput::GroundedEval {
            receipt: Box::new(receipt),
        },
        rows,
    ))
}
