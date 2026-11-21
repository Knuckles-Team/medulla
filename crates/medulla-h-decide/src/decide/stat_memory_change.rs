//! Memory-change evaluation served by `DecisionEval`
//! (MEDULLA-DECISION-ENGINE-R141.102).
//!
//! Two jobs share the decision job store: a fidelity-gated rewrite evaluation
//! (R141.100) and a benchmark receipt (R141.101). Each resolves the request's
//! named policy, runs the pure kernel and reports the sealed evaluation as the
//! job output. A receipt never changes memory: the only write is the job row
//! `submit_job` stores and the receipt row the admission gate reads
//! (MEDULLA-DECISION-ENGINE-R141.104).

use std::sync::Arc;

use medulla_decision_stats::decision::memory_benchmark::{benchmark, BenchmarkVerdict, RunMetrics};
use medulla_decision_stats::decision::memory_rewrite::evaluate_rewrite;
use medulla_decision_types::decision::memory_benchmark::{
    BenchmarkKnobs, BenchmarkVerdictKind, ChangeKind, MemoryBenchmarkReceipt,
    MemoryChangeBenchmarkRequest, RunMetricsReceipt, MEMORY_BENCHMARK_SCHEMA_VERSION,
    MEMORY_RECEIPT_NOT_ADMIT, MEMORY_RECEIPT_REQUIRED, MEMORY_RECEIPT_STALE,
};
use medulla_decision_types::decision::memory_rewrite::{
    MemoryRewriteEvaluation, MemoryRewriteKnobs, MemoryRewriteProposal, RewriteVerdict,
    MEMORY_REVISION_STALE,
};
use medulla_persistence_records::decision_jobs::{
    decode_artifact, encode_artifact, memory_receipt_key,
};
use medulla_decision_types::decision::statistical::StatisticalErrorCode;
use medulla_decision_types::decision::DecisionJobOutput;
use medulla_mutation_types::memory_sync::DocId;
use medulla_persistence_records::agent_library::AgentLibraryStore;
use medulla_server_api::authority_context::VerifiedRequestContext;
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
    let rows = vec![(
        memory_receipt_key(&evaluation.receipt_digest),
        encode_artifact(&StoredMemoryReceipt::Rewrite(evaluation.clone()))?,
    )];
    Ok((
        DecisionJobOutput::MemoryRewrite {
            evaluation: Box::new(evaluation),
        },
        rows,
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
    let rows = vec![benchmark_receipt_row(&receipt)?];
    Ok((
        DecisionJobOutput::MemoryBenchmark {
            receipt: Box::new(receipt),
        },
        rows,
    ))
}

/// The decision-artifact row that stores a benchmark receipt for the gate.
pub fn benchmark_receipt_row(
    receipt: &MemoryBenchmarkReceipt,
) -> Result<(String, Vec<u8>), String> {
    Ok((
        memory_receipt_key(&receipt.receipt_digest),
        encode_artifact(&StoredMemoryReceipt::Benchmark(receipt.clone()))?,
    ))
}

/// A memory-change receipt as the tenant's decision artifacts hold it, under
/// [`memory_receipt_key`]: the one row the admission gate reads.
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(tag = "receipt", rename_all = "snake_case")]
enum StoredMemoryReceipt {
    Benchmark(MemoryBenchmarkReceipt),
    Rewrite(MemoryRewriteEvaluation),
}

/// The change a memory mutation makes, as the gate checks it.
pub struct MemoryChange<'a> {
    pub record_id: &'a str,
    pub kind: ChangeKind,
    /// The content digest of the candidate the mutation writes.
    pub candidate_digest: &'a str,
    /// The head revision the mutation was planned at.
    pub head_revision: u64,
}

/// The admission gate of the memory mutation routes
/// (MEDULLA-DECISION-ENGINE-R141.104). A `Rewrite` needs one `Admit` rewrite
/// receipt and one `Admit` benchmark receipt; every other kind needs one
/// `Admit` benchmark receipt. Each is read in the caller's tenant only, so
/// another tenant's digest reads as absent, and must bind the change's record
/// id and kind, its candidate digest and its head revision. Answers the
/// premises `memory:receipt=<digest>` of the receipts that admitted the change;
/// a refusal writes nothing.
pub fn require_admit_receipts(
    store: &AgentLibraryStore,
    tenant_id: &str,
    change: &MemoryChange<'_>,
    digests: &[String],
) -> Result<Vec<String>, String> {
    let (mut benchmark, mut rewrite) = (None, None);
    let mut seen = Vec::new();
    for digest in digests {
        if seen.contains(&digest) {
            continue;
        }
        seen.push(digest);
        let Some(bytes) = store.decision_artifact(tenant_id, &memory_receipt_key(digest))? else {
            continue;
        };
        match decode_artifact::<StoredMemoryReceipt>(&bytes, "memory receipt")? {
            StoredMemoryReceipt::Benchmark(receipt)
                if receipt.record_id == change.record_id && receipt.change_kind == change.kind =>
            {
                benchmark = benchmark.or(Some(receipt));
            }
            StoredMemoryReceipt::Rewrite(evaluation)
                if evaluation.proposal.record_id == change.record_id =>
            {
                rewrite = rewrite.or(Some(evaluation));
            }
            // A receipt for another record or change kind never counts.
            _ => {}
        }
    }
    let mut premises = Vec::new();
    let Some(benchmark) = benchmark else {
        return Err(coded(
            MEMORY_RECEIPT_REQUIRED,
            "the change names no benchmark receipt of this record and change kind",
        ));
    };
    if benchmark.verdict != BenchmarkVerdictKind::Admit {
        return Err(coded(MEMORY_RECEIPT_NOT_ADMIT, "the benchmark receipt discards the change"));
    }
    if hex_digest(&benchmark.candidate.digest) != hex_digest(change.candidate_digest)
        || benchmark.baseline.revision != change.head_revision
    {
        return Err(coded(
            MEMORY_RECEIPT_STALE,
            "the benchmark receipt is bound to another candidate or head revision",
        ));
    }
    premises.push(format!("memory:receipt={}", benchmark.receipt_digest));
    if change.kind == ChangeKind::Rewrite {
        let Some(evaluation) = rewrite else {
            return Err(coded(
                MEMORY_RECEIPT_REQUIRED,
                "a rewrite names no rewrite receipt of this record",
            ));
        };
        if evaluation.verdict != RewriteVerdict::Admit {
            return Err(coded(MEMORY_RECEIPT_NOT_ADMIT, "the rewrite receipt discards the change"));
        }
        if hex_digest(&evaluation.proposal.candidate.digest) != hex_digest(change.candidate_digest)
            || evaluation.proposal.active.revision != change.head_revision
        {
            return Err(coded(
                MEMORY_RECEIPT_STALE,
                "the rewrite receipt is bound to another candidate or head revision",
            ));
        }
        premises.push(format!("memory:receipt={}", evaluation.receipt_digest));
    }
    Ok(premises)
}

/// The head revision a mutation on `graph` is planned at: the graph's version,
/// read behind the caller's read access. The mutation applies only while the
/// version is unchanged.
pub async fn planned_head_revision(
    state: &SharedState,
    verified: &VerifiedRequestContext,
    graph: &str,
) -> Result<u64, String> {
    super::topology_read::graph_core(state, graph, verified)
        .await
        .map(|core| core.version())
}
