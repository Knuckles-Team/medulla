//! MEDULLA-DECISION-ENGINE-R141.102: memory-change evaluation served by
//! `DecisionEval`. A rewrite proposal and a benchmark are bound to the stored
//! head, the resolved policy and the dataset; a receipt never changes memory.

use super::*;
use medulla_decision_types::decision::memory_benchmark::{
    BenchmarkDataset, BenchmarkQuery, ChangeKind, MemoryChangeBenchmarkRequest, QueryResult,
    RevisionRef as BenchRevision, RunResults, MEMORY_BENCHMARK_SCHEMA_VERSION,
};
use medulla_decision_types::decision::memory_rewrite::{
    HistoryEntry, MemoryRewriteProposal, RevisionRef, RewriteHistory, RewriteVerdict,
    MEMORY_REWRITE_SCHEMA_VERSION,
};
use medulla_decision_types::decision::DecisionJobStatusRequest;
use medulla_types::contract::Digest256;
use medulla_types::memory_sync::{DocId, RequestId, SyncAction, SyncRequest, SyncStamp};

const DOC: &str = "memory:served-1";
const GRAPH: &str = "default";

async fn seed_head(h: &Harness) -> u64 {
    let mut state = h.state.write().await;
    if state.registry.get(GRAPH).is_none() {
        state
            .registry
            .create_graph(
                GRAPH,
                medulla_protocol_types::protocol::GraphType::Global,
                None,
            )
            .expect("create the memory graph");
    }
    let core = state.registry.get(GRAPH).unwrap().core.clone();
    let stamp = SyncStamp {
        actor: "seeder".into(),
        policy_digest: Digest256::sha256(b"policy"),
        graph_incarnation: "incarnation-1".into(),
    };
    let mut expected = None;
    for (index, content) in ["first", "second"].into_iter().enumerate() {
        let request = SyncRequest {
            request_id: RequestId::new(format!("seed-{index}")).unwrap(),
            doc_id: DocId::new(DOC).unwrap(),
            action: SyncAction::Submit {
                expected_revision: expected,
                content: content.into(),
                content_digest: Digest256::sha256(content.as_bytes()),
            },
        };
        medulla_core::memory_snapshot_store::apply_sync(&core, &request, &stamp);
        expected = Some(index as u64 + 1);
    }
    medulla_core::memory_snapshot_store::load_head(&core, &DocId::new(DOC).unwrap())
        .expect("the seeded head")
        .revision
}

async fn head_digest(h: &Harness) -> Digest256 {
    let state = h.state.read().await;
    let core = state.registry.get(GRAPH).unwrap().core.clone();
    medulla_core::memory_snapshot_store::load_head(&core, &DocId::new(DOC).unwrap())
        .expect("the head")
        .content_digest
}

fn signals(count: usize) -> Vec<String> {
    (0..count).map(|i| format!("signal number {i}")).collect()
}

fn policy_digest(h: &Harness) -> String {
    let resolved = super::super::stat_support::resolve_policy(
        h.store.as_ref(),
        TENANT,
        &DecisionPolicyRef::Default,
    )
    .unwrap();
    resolved.digest.trim_start_matches("sha256:").to_string()
}

fn proposal(h: &Harness, key: &str, kept: usize, active_revision: u64) -> MemoryRewriteProposal {
    MemoryRewriteProposal {
        schema_version: MEMORY_REWRITE_SCHEMA_VERSION,
        tenant_id: TENANT.into(),
        idempotency_key: key.into(),
        graph: GRAPH.into(),
        record_id: DOC.into(),
        active: RevisionRef {
            revision: active_revision,
            digest: "a".repeat(64),
        },
        candidate: RevisionRef {
            revision: 3,
            digest: "b".repeat(64),
        },
        active_signals: signals(20),
        candidate_signals: signals(kept),
        token_count: 40,
        metric_id: "memory_fidelity_set_overlap".into(),
        policy: policy_digest(h),
        policy_ref: DecisionPolicyRef::Default,
        history: RewriteHistory::sealed(Vec::<HistoryEntry>::new()).unwrap(),
    }
}

fn rev(revision: u64, fill: &str) -> BenchRevision {
    BenchRevision {
        revision,
        digest: fill.repeat(64),
    }
}

fn benchmark_request(key: &str, candidate_hits: bool) -> MemoryChangeBenchmarkRequest {
    let answer = |i: usize| format!("answer {i}");
    let queries: Vec<BenchmarkQuery> = (0..4)
        .map(|i| BenchmarkQuery {
            query_id: format!("q{i}"),
            expected_evidence_ids: vec![format!("e{i}")],
            reference_answer_digest:
                medulla_decision_stats::decision::memory_benchmark::answer_digest(&answer(i)),
        })
        .collect();
    let run = |revision: BenchRevision, hit: bool| RunResults {
        revision,
        results: (0..4)
            .map(|i| QueryResult {
                query_id: format!("q{i}"),
                top_ids: if hit {
                    vec![format!("e{i}")]
                } else {
                    vec!["none".into()]
                },
                answer_text: if hit { answer(i) } else { "wrong".into() },
            })
            .collect(),
    };
    MemoryChangeBenchmarkRequest {
        schema_version: MEMORY_BENCHMARK_SCHEMA_VERSION,
        tenant_id: TENANT.into(),
        idempotency_key: key.into(),
        record_id: DOC.into(),
        change_kind: ChangeKind::Rewrite,
        baseline: rev(2, "a"),
        candidate: rev(3, "b"),
        dataset: BenchmarkDataset {
            id: "ds".into(),
            revision: 1,
            digest: "d".repeat(64),
            queries,
        },
        k: 3,
        evaluator_revision: "eval-1".into(),
        policy: DecisionPolicyRef::Default,
        baseline_results: run(rev(2, "a"), true),
        candidate_results: run(rev(3, "b"), candidate_hits),
    }
}

async fn send(h: &Harness, op: DecisionEvalOp) -> Result<DecisionJobRecord, String> {
    decode(super::super::jobs::handle_decision_eval(&h.state, 51, &verified(), op).await)
}

async fn rewrite(h: &Harness, request: MemoryRewriteProposal) -> DecisionJobRecord {
    let op = DecisionEvalOp::MemoryRewriteSubmit {
        request: Box::new(request),
    };
    send(h, op).await.unwrap()
}

async fn benchmark(h: &Harness, request: MemoryChangeBenchmarkRequest) -> DecisionJobRecord {
    let op = DecisionEvalOp::MemoryChangeBenchmarkSubmit {
        request: Box::new(request),
    };
    send(h, op).await.unwrap()
}

fn evaluation(
    job: &DecisionJobRecord,
) -> &medulla_decision_types::decision::memory_rewrite::MemoryRewriteEvaluation {
    match succeeded(job) {
        DecisionJobOutput::MemoryRewrite { evaluation } => evaluation,
        other => panic!("a rewrite output, got {other:?}"),
    }
}

fn receipt(
    job: &DecisionJobRecord,
) -> &medulla_decision_types::decision::memory_benchmark::MemoryBenchmarkReceipt {
    match succeeded(job) {
        DecisionJobOutput::MemoryBenchmark { receipt } => receipt,
        other => panic!("a benchmark output, got {other:?}"),
    }
}

fn failure(job: &DecisionJobRecord) -> &str {
    match &job.state {
        DecisionJobState::Failed { code } => code,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

// spec: MEDULLA-DECISION-ENGINE-R141.102
#[tokio::test]
async fn rewrite_admit_and_discard_through_decision_eval() {
    let h = Harness::new().await;
    assert_eq!(seed_head(&h).await, 2);
    let head_before = head_digest(&h).await;

    // An admitting rewrite keeps every signal and is stored as a job.
    let admit = rewrite(&h, proposal(&h, "rw-1", 20, 2)).await;
    let admitted = evaluation(&admit);
    assert_eq!(admitted.verdict, RewriteVerdict::Admit);
    assert_eq!(admitted.fidelity_q4, 10_000);
    let premises = admitted.premises(
        &medulla_decision_types::decision::memory_rewrite::MemoryRewriteKnobs::from_policy(
            &medulla_decision_types::decision::DecisionPolicy::engine_default(),
        ),
    );
    assert!(premises
        .iter()
        .any(|p| p == "memory:record=memory:served-1"));

    // The receipt reads back through Status.
    let status = send(
        &h,
        DecisionEvalOp::Status {
            request: DecisionJobStatusRequest {
                tenant_id: TENANT.into(),
                job_id: admit.job_id.clone(),
            },
        },
    )
    .await
    .unwrap();
    assert_eq!(evaluation(&status), admitted);

    // A rewrite that drops most signals is discarded with its reason.
    let discard = rewrite(&h, proposal(&h, "rw-2", 5, 2)).await;
    let discarded = evaluation(&discard);
    assert_eq!(discarded.verdict, RewriteVerdict::Discard);
    assert!(!discarded.reason_codes.is_empty());

    // A stale active revision, an unknown record and a foreign policy digest
    // each refuse by code and store no evaluation.
    let stale = rewrite(&h, proposal(&h, "rw-3", 20, 1)).await;
    assert!(failure(&stale).starts_with("MEMORY_REVISION_STALE"));
    let mut unknown = proposal(&h, "rw-4", 20, 2);
    unknown.record_id = "memory:never-written".into();
    assert!(failure(&rewrite(&h, unknown).await).starts_with("MEMORY_REVISION_STALE"));
    let mut foreign = proposal(&h, "rw-5", 20, 2);
    foreign.policy = "f".repeat(64);
    assert!(failure(&rewrite(&h, foreign).await).contains("COMPONENT_PIN_MISMATCH"));

    // A repeated key with new content is an idempotency conflict; the same
    // content replays the stored job.
    let replay = rewrite(&h, proposal(&h, "rw-1", 20, 2)).await;
    assert_eq!(replay.job_id, admit.job_id);
    let conflict = send(
        &h,
        DecisionEvalOp::MemoryRewriteSubmit {
            request: Box::new(proposal(&h, "rw-1", 5, 2)),
        },
    )
    .await;
    assert!(conflict.unwrap_err().contains("IDEMPOTENCY_CONFLICT"));

    // Benchmarks: an admit, a discard with its reason, and the refusals.
    let good = benchmark(&h, benchmark_request("bm-1", true)).await;
    let good = receipt(&good);
    assert_eq!(good.reason_codes, Vec::<String>::new());
    assert_eq!(good.candidate_metrics.queries, 4);
    assert!(good
        .premises
        .iter()
        .any(|p| p.starts_with("memory:dataset=")));
    assert_eq!(good.receipt_digest, good.compute_receipt_digest());
    let bad = benchmark(&h, benchmark_request("bm-2", false)).await;
    assert_eq!(
        receipt(&bad).reason_codes.first().map(String::as_str),
        Some("RECALL_BELOW_MIN")
    );
    let mut snapshot = benchmark_request("bm-3", true);
    snapshot.candidate_results.revision = rev(9, "c");
    let mismatch = benchmark(&h, snapshot).await;
    assert!(failure(&mismatch).starts_with("MEMORY_SNAPSHOT_MISMATCH"));
    let mut empty = benchmark_request("bm-4", true);
    empty.dataset.queries.clear();
    assert!(failure(&benchmark(&h, empty).await).starts_with("MEMORY_DATASET_EMPTY"));

    // Another tenant's caller is refused at the route, and nothing changed
    // the stored head.
    let foreign_caller =
        medulla_server_api::authority_context::VerifiedRequestContext::verified_for_test_in_tenant(
            "intruder",
            "tenant-foreign",
        );
    let response = super::super::jobs::handle_decision_eval(
        &h.state,
        52,
        &foreign_caller,
        DecisionEvalOp::MemoryRewriteSubmit {
            request: Box::new(proposal(&h, "rw-6", 20, 2)),
        },
    )
    .await;
    assert!(decode::<DecisionJobRecord>(response)
        .unwrap_err()
        .starts_with("ACCESS_DENIED"));
    assert_eq!(head_digest(&h).await, head_before);
}
