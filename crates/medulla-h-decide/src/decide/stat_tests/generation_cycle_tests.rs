//! The generation cycle (MEDULLA-DECISION-ENGINE-R174), served: a model is
//! published, the shadow graph is re-embedded in checkpointed batches under a
//! background capacity lease, the result is evaluated against the active
//! generation and the logical graph's pointer moves with the passing receipt.

use std::sync::atomic::{AtomicUsize, Ordering};

use super::learning_tests::{axis, graph_with, plus, q16, space_of};
use super::retrieval_tests::{
    attest, clone_executed, evaluate_by, executed_template, learn, outcome, sql,
};
use super::*;
use medulla_decision_types::decision::statistical::log::DecisionLogOp;
use medulla_decision_types::decision::statistical::retrieval::{LearningRecorded, LearningWrite};
use medulla_decision_types::decision::statistical::retrieval_generation::{
    GenerationCycleJob, GenerationCycleRequest, GenerationCycleState, GenerationEvalItem,
    GenerationEvalRequest, ModelRef,
};
use medulla_exec::exec::TextEmbedder;
use medulla_mutation_types::{capacity_lease::CapacityCell, capacity_lease::CapacityResourceClass};
use medulla_persistence::redb_backend::RedbBackend;
use medulla_protocol_types::protocol::Method;
use medulla_server_api::api::persistence::PersistenceBackend;
use medulla_work_types::native_control::{
    CapacityCellUpdateRequest, CapacityStatusRequest, NativeControlSchemaVersion,
};
use serde_json::json;

use super::super::stat_executor::ExecutionContext;
use super::super::stat_generation::Generations;
use super::super::stat_generation_cycle::{run_cycle, CycleRun, Reembedder};
use super::super::stat_log::LogReader;
use super::super::stat_vectors::graph_vectors;

const ACTIVE: &str = "kg-cycle";
const SHADOW: &str = "kg-cycle-2";
pub(super) const CELL: &str = "cell-embed";
/// Judged runs, and the shadow nodes they cite or reject.
const JUDGED: usize = 6;
/// Shadow nodes: the judged ones plus fillers; two default batches of 256.
const SHADOW_NODES: usize = 300;
const SHADOW_DIMS: usize = 384;

/// A deterministic embedder that counts how many texts it embedded.
struct Counting(AtomicUsize);

impl Reembedder for Counting {
    fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, String> {
        self.0.fetch_add(texts.len(), Ordering::SeqCst);
        let hash = medulla_exec::exec::HashEmbedder::new(SHADOW_DIMS);
        texts.iter().map(|text| hash.embed(text)).collect()
    }
}

fn text_of(index: usize) -> String {
    match index {
        i if i < JUDGED => format!("p{i}"),
        i if i < 2 * JUDGED => format!("n{}", i - JUDGED),
        i => format!("filler-{i}"),
    }
}

/// The active generation (8 dimensions, embedded) and the shadow generation
/// (384 dimensions, its nodes present and not yet embedded).
async fn two_graphs(h: &Harness) {
    let rows = (0..JUDGED).flat_map(|i| {
        [
            (format!("p{i}"), axis(8, i, 1.0)),
            (format!("n{i}"), plus(axis(8, 6, 1.0), &axis(8, i, 0.1))),
        ]
    });
    graph_with(h, ACTIVE, "1", 8, rows.collect()).await;
    let core = graph_with(h, SHADOW, "2", SHADOW_DIMS, Vec::new()).await;
    for index in 0..SHADOW_NODES {
        let id = text_of(index);
        let props = json!({"type": "Doc", "text": id});
        core.add_node(id, rmp_serde::to_vec_named(&props).unwrap());
    }
}

/// The judged runs the receipt replays, each query embedded in both spaces.
async fn judged_items(h: &Harness) -> Vec<GenerationEvalItem> {
    let template = executed_template(h).await;
    let hash = medulla_exec::exec::HashEmbedder::new(SHADOW_DIMS);
    let mut items = Vec::new();
    for i in 0..JUDGED {
        let record_id = format!("cycle-{i}");
        clone_executed(h, &template, &record_id);
        let (n, p) = (format!("n{i}"), format!("p{i}"));
        let recorded = learn(h, "decider", attest(outcome(&record_id, &[&n, &p], &[&p]))).await;
        assert!(recorded.is_ok(), "{recorded:?}");
        evaluate_by(h, "evaluator", &record_id, true).await;
        let active = plus(axis(8, 6, 1.0), &axis(8, i, 0.5));
        let shadow = hash.embed(&p).unwrap();
        let to_q16 = |v: Vec<f32>| q16(&v.into_iter().map(f64::from).collect::<Vec<_>>());
        items.push(GenerationEvalItem {
            record_id,
            active_q16: to_q16(active),
            shadow_q16: to_q16(shadow),
        });
    }
    items
}

fn request(items: Vec<GenerationEvalItem>, max_batches: Option<u32>) -> GenerationCycleRequest {
    GenerationCycleRequest {
        model_ref: ModelRef {
            id: "m".to_string(),
            revision: "2".to_string(),
            dimension: SHADOW_DIMS as u32,
        },
        evaluation: GenerationEvalRequest {
            logical: ACTIVE.to_string(),
            active_graph: ACTIVE.to_string(),
            active_space: space_of("1", 8),
            shadow_graph: SHADOW.to_string(),
            shadow_space: space_of("2", SHADOW_DIMS),
            items: BoundedVec::new(items).unwrap(),
            top_k: 5,
            min_eval_items: 5,
            max_score_psi_q16: 64 << 16,
        },
        capacity_cell: CELL.to_string(),
        max_batches,
        index_adapter: None,
    }
}

async fn cycle(
    h: &Harness,
    request: &GenerationCycleRequest,
) -> Result<Box<GenerationCycleJob>, String> {
    let write = LearningWrite::RunGenerationCycle {
        request: Box::new(request.clone()),
    };
    match learn(h, "decider", write).await? {
        LearningRecorded::Cycle(job) => Ok(job),
        other => panic!("cycle: {other:?}"),
    }
}

/// A durable capacity ledger with one embedding cell, installed in the served
/// state. The returned backend is the ledger the lease is read from.
async fn with_ledger(h: &Harness, dir: &std::path::Path) -> std::sync::Arc<RedbBackend> {
    with_ledger_class(h, dir, CapacityResourceClass::LlmEmbedding).await
}

/// A durable capacity ledger whose one cell is of `class`.
pub(super) async fn with_ledger_class(
    h: &Harness,
    dir: &std::path::Path,
    class: CapacityResourceClass,
) -> std::sync::Arc<RedbBackend> {
    let backend = std::sync::Arc::new(
        RedbBackend::open_with_shards(dir.to_str().unwrap().to_string(), 16, 1).unwrap(),
    );
    backend
        .commit_capacity_lease(
            &medulla_kernel::persist::sanitize(ACTIVE),
            Method::UpdateCapacityCell {
                request: CapacityCellUpdateRequest {
                    schema_version: NativeControlSchemaVersion::V1,
                    cell: CapacityCell {
                        cell_id: CELL.into(),
                        parent_id: None,
                        resource_class: class,
                        capacity: 8,
                        reserved_floor: 0,
                        epoch: 1,
                        policy_digest: "d".repeat(64),
                        updated_at_ms: 0,
                        throttle: None,
                    },
                    expected_epoch: None,
                    now_ms: 1,
                },
            },
        )
        .await
        .unwrap();
    h.state.write().await.persistence = Some(backend.clone());
    backend
}

async fn pointer_rows(h: &Harness) -> Vec<Vec<serde_json::Value>> {
    sql(
        h,
        "decider",
        "SELECT transition, active FROM decision_pointers ORDER BY seq",
    )
    .await
}

/// Verifies MEDULLA-DECISION-ENGINE-R174.1 and R174.2: a cycle publishes,
/// re-embeds in checkpointed batches, evaluates and activates exactly once.
// spec: MEDULLA-DECISION-ENGINE-R174.1
// spec: MEDULLA-DECISION-ENGINE-R174.2
// spec: MEDULLA-DECISION-ENGINE-R174.6
#[tokio::test]
async fn cycle_publishes_reembeds_evaluates_and_activates() {
    #[cfg(any(feature = "security", feature = "raft"))]
    let _env_guard = medulla_kernel::crypto::acquire_test_env_read_lock().await;
    let h = Harness::with_isolation(ServerState::test_isolation("decider")).await;
    let ledger = tempfile::tempdir().unwrap();
    with_ledger(&h, ledger.path()).await;
    two_graphs(&h).await;
    let items = judged_items(&h).await;

    let first = cycle(&h, &request(items.clone(), Some(1))).await.unwrap();
    assert_eq!(first.cycle.state(), GenerationCycleState::Reembedding);
    assert_eq!(
        (first.batches, first.reembedded_nodes, first.next_node),
        (1, 256, 256)
    );
    assert!(first.receipt.is_none());
    assert!(pointer_rows(&h).await.is_empty(), "nothing is served yet");

    let done = cycle(&h, &request(items.clone(), None)).await.unwrap();
    assert_eq!(done.cycle.state(), GenerationCycleState::Activated);
    assert_eq!(
        (done.batches, done.reembedded_nodes),
        (2, SHADOW_NODES as u64)
    );
    assert_eq!(
        done.job_id, first.job_id,
        "one job per graph and model revision"
    );
    assert_eq!(
        pointer_rows(&h).await,
        vec![vec![json!("activated"), json!(true)]]
    );

    let replay = cycle(&h, &request(items, None)).await.unwrap();
    assert_eq!(*replay, *done, "a finished cycle is a replay");
    assert_eq!(pointer_rows(&h).await.len(), 1, "the pointer moved once");
}

/// Verifies MEDULLA-DECISION-ENGINE-R174.2: a resumed cycle embeds each node
/// once and activates once.
// spec: MEDULLA-DECISION-ENGINE-R174.2
#[tokio::test]
async fn resume_activates_once() {
    let h = Harness::with_isolation(ServerState::test_isolation("decider")).await;
    two_graphs(&h).await;
    let items = judged_items(&h).await;
    let verified = VerifiedRequestContext::verified_for_test_in_tenant("decider", TENANT);
    let reader = LogReader::of(&verified);
    let active = graph_vectors(&h.state, "decider", ACTIVE).await.unwrap();
    let shadow = graph_vectors(&h.state, "decider", SHADOW).await.unwrap();
    let gens = Generations {
        active: &active,
        shadow: &shadow,
    };
    let ctx = ExecutionContext {
        store: &h.store,
        tenant_id: TENANT,
        now_ms: 1_000,
        server_secret: b"cycle-test-secret",
        matched: &[],
        designation: None,
        reader: None,
    };
    let embedder = Counting(AtomicUsize::new(0));
    let run = CycleRun {
        ctx: &ctx,
        reader: &reader,
        gens: &gens,
        embedder: &embedder,
        index: None,
        tenant: TENANT,
        lease_id: None,
        batch_nodes: 100,
    };
    let request = request(items, Some(1));
    let mut states = Vec::new();
    for _ in 0..6 {
        let LearningRecorded::Cycle(job) = run_cycle(&run, &request).unwrap() else {
            panic!("a cycle answers with its job");
        };
        states.push(job.cycle.state());
    }
    assert_eq!(
        states,
        vec![
            GenerationCycleState::Reembedding,
            GenerationCycleState::Reembedding,
            GenerationCycleState::Activated,
            GenerationCycleState::Activated,
            GenerationCycleState::Activated,
            GenerationCycleState::Activated,
        ],
        "100 + 100 + 100 nodes, then evaluation and activation on the third resume"
    );
    assert_eq!(
        embedder.0.load(Ordering::SeqCst),
        SHADOW_NODES,
        "a resume continues from the checkpoint and never re-embeds a node"
    );
    assert_eq!(
        pointer_rows(&h).await,
        vec![vec![json!("activated"), json!(true)]]
    );
}

/// Verifies MEDULLA-DECISION-ENGINE-R174.3: a submitted cycle creates its job
/// and holds a background capacity lease from before its first batch, and
/// another tenant sees neither.
// spec: MEDULLA-DECISION-ENGINE-R174.3
#[tokio::test]
async fn submit_creates_a_job_and_holds_a_lease() {
    #[cfg(any(feature = "security", feature = "raft"))]
    let _env_guard = medulla_kernel::crypto::acquire_test_env_read_lock().await;
    let h = Harness::with_isolation(ServerState::test_isolation("decider")).await;
    let ledger = tempfile::tempdir().unwrap();
    let backend = with_ledger(&h, ledger.path()).await;
    two_graphs(&h).await;
    let items = judged_items(&h).await;

    let job = cycle(&h, &request(items.clone(), Some(1))).await.unwrap();
    assert_eq!(job.cycle.state(), GenerationCycleState::Reembedding);
    let lease = job.lease_id.clone().expect("the job holds a lease");
    let status = backend
        .read_capacity_status(
            &medulla_kernel::persist::sanitize(ACTIVE),
            &CapacityStatusRequest {
                schema_version: NativeControlSchemaVersion::V1,
                tenant_ref: TENANT.into(),
                cell_id: Some(CELL.into()),
                lease_id: None,
                max_count: 10,
                cursor: None,
            },
        )
        .await
        .unwrap();
    assert!(
        format!("{status:?}").contains(&lease),
        "the ledger holds the job's lease: {status:?}"
    );
    let key = super::super::stat_generation_cycle::job_key(&job.job_id);
    assert!(h.store.decision_artifact(TENANT, &key).unwrap().is_some());
    assert!(
        h.store
            .decision_artifact("other-tenant", &key)
            .unwrap()
            .is_none(),
        "another tenant has no such job"
    );

    let foreign = DecisionLogOp::Learn {
        tenant_id: "other-tenant".to_string(),
        write: LearningWrite::RunGenerationCycle {
            request: Box::new(request(items, Some(1))),
        },
    };
    let refused =
        decode::<LearningRecorded>(super::log_tests::log_op(&h, "decider", foreign).await);
    assert!(refused.unwrap_err().starts_with("ACCESS_DENIED"));
}

/// Verifies MEDULLA-DECISION-ENGINE-R174.5: the receipt of an activated cycle
/// holds the model revision, the re-embedded node count, the comparison
/// metrics and the pointer before and after.
// spec: MEDULLA-DECISION-ENGINE-R174.5
#[tokio::test]
async fn receipt_holds_model_counts_metrics_and_pointers() {
    #[cfg(any(feature = "security", feature = "raft"))]
    let _env_guard = medulla_kernel::crypto::acquire_test_env_read_lock().await;
    let h = Harness::with_isolation(ServerState::test_isolation("decider")).await;
    let ledger = tempfile::tempdir().unwrap();
    with_ledger(&h, ledger.path()).await;
    two_graphs(&h).await;
    let items = judged_items(&h).await;

    let job = cycle(&h, &request(items, None)).await.unwrap();
    let receipt = job.receipt.expect("an activated cycle holds its receipt");
    assert_eq!(receipt.model_ref.revision, "2");
    assert_eq!(receipt.reembedded_nodes, SHADOW_NODES as u64);
    let metrics = &receipt.evaluation.receipt;
    assert!(metrics.passed, "{metrics:?}");
    assert_eq!(
        (metrics.n_eval, metrics.wins),
        (JUDGED as u64, JUDGED as u64)
    );
    assert!(metrics.shadow_mrr_q16 > metrics.active_mrr_q16);
    assert_eq!(metrics.shadow_embedded, SHADOW_NODES as u64);
    assert_eq!(
        job.cycle.receipt_digest.as_deref(),
        Some(receipt.evaluation.receipt_digest.as_str())
    );
    assert_eq!(receipt.pointer_before, None);
    assert_eq!(receipt.pointer_after.as_deref(), Some(SHADOW));
}

/// Moves the logical graph's pointer to another generation while the cycle
/// re-embeds, as a concurrent operator would.
struct MovesPointer<'a>(&'a medulla_persistence_records::agent_library::AgentLibraryStore);

impl Reembedder for MovesPointer<'_> {
    fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, String> {
        use super::super::stat_generation::generation_pointer;
        use super::super::stat_pointer::{activate, PointerMove, Qualified};
        let at = PointerMove {
            tenant: TENANT,
            key: generation_pointer(ACTIVE),
            principal: "operator",
            now_ms: 5,
        };
        let other = Qualified {
            target: "kg-other".to_string(),
            receipt_digest: "sha256:other".to_string(),
        };
        activate(self.0, &at, &other)?;
        let hash = medulla_exec::exec::HashEmbedder::new(SHADOW_DIMS);
        texts.iter().map(|text| hash.embed(text)).collect()
    }
}

/// Verifies MEDULLA-DECISION-ENGINE-R174.4: each refusal returns its code and
/// leaves the active pointer unchanged.
// spec: MEDULLA-DECISION-ENGINE-R174.4
#[tokio::test]
async fn refusal_codes() {
    #[cfg(any(feature = "security", feature = "raft"))]
    let _env_guard = medulla_kernel::crypto::acquire_test_env_read_lock().await;
    let h = Harness::with_isolation(ServerState::test_isolation("decider")).await;
    let ledger = tempfile::tempdir().unwrap();
    with_ledger(&h, ledger.path()).await;
    two_graphs(&h).await;
    let items = judged_items(&h).await;
    let declared = medulla_protocol_types::error_registry::declared_error_code;

    // A caller without admin:decision-head.
    let stranger = DecisionLogOp::Learn {
        tenant_id: TENANT.to_string(),
        write: LearningWrite::RunGenerationCycle {
            request: Box::new(request(items.clone(), None)),
        },
    };
    let refused =
        decode::<LearningRecorded>(super::log_tests::log_op(&h, "stranger", stranger).await);
    assert!(refused.unwrap_err().starts_with("ACCESS_DENIED"));

    // A model of another dimension than the shadow generation.
    let mut wrong = request(items.clone(), None);
    wrong.model_ref.dimension = 8;
    let refused = cycle(&h, &wrong).await.unwrap_err();
    assert!(
        refused.starts_with("GENERATION_DIMENSION_MISMATCH"),
        "{refused}"
    );
    assert!(declared("GENERATION_DIMENSION_MISMATCH"));

    // A failing receipt ends the cycle `rejected`, with its code recorded.
    let mut failing = request(items.clone(), None);
    failing.evaluation.min_eval_items = 100;
    let rejected = cycle(&h, &failing).await.unwrap();
    assert_eq!(rejected.cycle.state(), GenerationCycleState::Rejected);
    assert_eq!(
        rejected.rejection.as_deref(),
        Some("GENERATION_RECEIPT_FAILED")
    );
    assert!(rejected.receipt.is_none());
    assert!(declared("GENERATION_RECEIPT_FAILED"));
    assert!(
        pointer_rows(&h).await.is_empty(),
        "no refusal moved the pointer"
    );

    // A pointer that moved during the cycle is a conflict, never overwritten.
    let verified = VerifiedRequestContext::verified_for_test_in_tenant("decider", TENANT);
    let reader = LogReader::of(&verified);
    let active = graph_vectors(&h.state, "decider", ACTIVE).await.unwrap();
    let shadow = graph_vectors(&h.state, "decider", SHADOW).await.unwrap();
    let gens = Generations {
        active: &active,
        shadow: &shadow,
    };
    let ctx = ExecutionContext {
        store: &h.store,
        tenant_id: TENANT,
        now_ms: 1_000,
        server_secret: b"cycle-test-secret",
        matched: &[],
        designation: None,
        reader: None,
    };
    let embedder = MovesPointer(&h.store);
    let run = CycleRun {
        ctx: &ctx,
        reader: &reader,
        gens: &gens,
        embedder: &embedder,
        index: None,
        tenant: TENANT,
        lease_id: None,
        batch_nodes: 512,
    };
    let mut racing = request(items, None);
    racing.model_ref.revision = "3".to_string();
    let conflict = match run_cycle(&run, &racing) {
        Err(error) => error,
        Ok(_) => panic!("a moved pointer is refused"),
    };
    assert!(
        conflict.starts_with("GENERATION_POINTER_CONFLICT"),
        "{conflict}"
    );
    assert!(declared("GENERATION_POINTER_CONFLICT"));
    assert_eq!(
        pointer_rows(&h).await,
        vec![vec![json!("activated"), json!(true)]],
        "only the operator's move is recorded"
    );
    let (_, pointer) = super::super::stat_pointer::read_pointer(
        &h.store,
        TENANT,
        &super::super::stat_generation::generation_pointer(ACTIVE),
    )
    .unwrap();
    assert_eq!(pointer.target(), Some("kg-other"));
}
