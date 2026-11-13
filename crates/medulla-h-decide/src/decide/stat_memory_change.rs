//! Memory-change evaluation served by `DecisionEval`
//! (MEDULLA-DECISION-ENGINE-R141.102).
//!
//! Two jobs share the decision job store: a fidelity-gated rewrite evaluation
//! (R141.100) and a benchmark receipt (R141.101). Each resolves the request's
//! named policy, runs the pure kernel and reports the sealed evaluation as the
//! job output. A receipt never changes memory: the only write is the job row
//! `submit_job` stores.

use std::sync::Arc;

use medulla_decision_stats::decision::memory_benchmark::{benchmark, BenchmarkVerdict, RunMetrics};
use medulla_decision_stats::decision::memory_rewrite::evaluate_rewrite;
use medulla_decision_types::decision::memory_benchmark::{
    BenchmarkKnobs, BenchmarkVerdictKind, MemoryBenchmarkReceipt, MemoryChangeBenchmarkRequest,
    RunMetricsReceipt, MEMORY_BENCHMARK_SCHEMA_VERSION,
};
use medulla_decision_types::decision::memory_rewrite::{
    MemoryRewriteKnobs, MemoryRewriteProposal, MEMORY_REVISION_STALE,
};
use medulla_decision_types::decision::statistical::StatisticalErrorCode;
use medulla_decision_types::decision::DecisionJobOutput;
use medulla_persistence_records::agent_library::AgentLibraryStore;
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_types::memory_sync::DocId;
use medulla_types::refusal_code::RefusalCode;

use super::stat_jobs::JobRun;
use super::stat_support::{refusal, resolve_policy};
use super::SharedState;

fn coded(code: RefusalCode, detail: impl std::fmt::Display) -> String {
    format!("{}: {detail}", code.as_str())
}

/// The stored head revision of the memory record a rewrite names, read from
/// the proposal's graph behind the caller's read access. An unknown record, an
/// unreadable graph and a head of another revision all answer
/// `MEMORY_REVISION_STALE`, so a refusal never confirms that a record exists.
pub(super) async fn stored_head_revision(
    state: &SharedState,
    verified: &VerifiedRequestContext,
    proposal: &MemoryRewriteProposal,
) -> Option<u64> {
    let doc = DocId::new(proposal.record_id.clone()).ok()?;
    let core: Arc<medulla_graph_core::graph::GraphCore> =
        super::topology_read::graph_core(state, &proposal.graph, verified)
            .await
            .ok()?;
    medulla_core::memory_snapshot_store::load_head(&core, &doc).map(|head| head.revision)
}

fn hex_digest(digest: &str) -> &str {
    digest.trim_start_matches("sha256:")
}

/// Evaluate one rewrite against the head revision read for it.
pub(super) fn run_rewrite(
    store: &AgentLibraryStore,
    proposal: &MemoryRewriteProposal,
    head_revision: Option<u64>,
) -> JobRun {
    let resolved = resolve_policy(store, &proposal.tenant_id, &proposal.policy_ref)?;
    if !proposal.policy.trim().is_empty() && hex_digest(&resolved.digest) != proposal.policy {
        return Err(refusal(
            StatisticalErrorCode::ComponentPinMismatch,
            "the proposal's policy digest differs from the resolved policy",
        ));
    }
    let knobs = MemoryRewriteKnobs::from_policy(&resolved.policy);
    let Some(head_revision) = head_revision else {
        return Err(coded(
            MEMORY_REVISION_STALE,
            "the record has no stored head at the active revision",
        ));
    };
    let evaluation = evaluate_rewrite(proposal, &knobs, head_revision)
        .map_err(|code| coded(code, "the rewrite proposal was refused"))?;
    Ok((
        DecisionJobOutput::MemoryRewrite {
            evaluation: Box::new(evaluation),
        },
        Vec::new(),
    ))
}

fn metrics_receipt(metrics: &RunMetrics) -> RunMetricsReceipt {
    RunMetricsReceipt {
        recall_ppm_sum: metrics.recall_ppm_sum,
        correct: metrics.correct,
        queries: metrics.queries,
        recall_at_k_ppm: metrics.recall_at_k_ppm,
        answer_accuracy_ppm: metrics.answer_accuracy_ppm,
    }
}

/// Score one benchmark under the request's named policy.
pub(super) fn run_benchmark(
    store: &AgentLibraryStore,
    request: &MemoryChangeBenchmarkRequest,
) -> JobRun {
    let resolved = resolve_policy(store, &request.tenant_id, &request.policy)?;
    let outcome = benchmark(request, BenchmarkKnobs::from_policy(&resolved.policy))
        .map_err(|refused| coded(refused.code, refused.detail))?;
    let (verdict, reason_codes) = match &outcome.verdict {
        BenchmarkVerdict::Admit => (BenchmarkVerdictKind::Admit, Vec::new()),
        BenchmarkVerdict::Discard { reasons } => (
            BenchmarkVerdictKind::Discard,
            reasons
                .iter()
                .map(|code| code.as_str().to_string())
                .collect(),
        ),
    };
    let mut receipt = MemoryBenchmarkReceipt {
        schema_version: MEMORY_BENCHMARK_SCHEMA_VERSION,
        tenant_id: request.tenant_id.clone(),
        record_id: request.record_id.clone(),
        change_kind: request.change_kind,
        baseline: request.baseline.clone(),
        candidate: request.candidate.clone(),
        dataset_id: request.dataset.id.clone(),
        dataset_revision: request.dataset.revision,
        dataset_digest: request.dataset.digest.clone(),
        k: request.k,
        evaluator_revision: request.evaluator_revision.clone(),
        policy_digest: hex_digest(&resolved.digest).to_string(),
        baseline_metrics: metrics_receipt(&outcome.baseline),
        candidate_metrics: metrics_receipt(&outcome.candidate),
        verdict,
        reason_codes,
        premises: outcome.premises,
        receipt_digest: String::new(),
    };
    receipt.receipt_digest = receipt.compute_receipt_digest();
    Ok((
        DecisionJobOutput::MemoryBenchmark {
            receipt: Box::new(receipt),
        },
        Vec::new(),
    ))
}
