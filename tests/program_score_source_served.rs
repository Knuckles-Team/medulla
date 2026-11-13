//! MEDULLA-DECISION-ENGINE-R813.2 / R813.3: a served `ProgramOptimize` submit
//! builds its corpus from the caller's stored `:Score` rows through
//! `corpus_from_scores`. The client names the graph only; examples it supplies
//! are refused, and a graph with no scores yields no corpus.

#![cfg(all(
    feature = "program-optimization",
    feature = "jobs",
    feature = "redb",
    feature = "security"
))]

mod common;
#[path = "common/test_support.rs"]
mod test_support;

use std::sync::Arc;

use medulla_modality::{Classification, OpaqueRef, PolicyEnvelope, PrivacyAttestation};
use medulla_persistence::redb_backend::RedbBackend;
use medulla_program::{
    AdapterKind, EvaluationSummary, FieldRole, FieldSpec, ModuleKind, OptimizationBudget,
    OptimizationRequest, OptimizerKind, ProgramModality, ProgramRevision, PromotionPolicy,
    ScoreSource, SignatureSpec, TrainingCorpus, PROGRAM_SCHEMA_VERSION,
};
use medulla_protocol_types::protocol::{Method, Response};
use medulla_server_api::api::persistence::PersistenceBackend;
use medulla_work_types::{jobs::JobKind, jobs::JobOp, jobs::SubmitJobSpec};
use sha2::{Digest, Sha256};

const SECRET: &str = "program-score-source-secret";
const GRAPH: &str = "__commons__";

fn opaque(namespace: &str, label: &str) -> OpaqueRef {
    OpaqueRef::scoped(namespace, &hex::encode(Sha256::digest(label.as_bytes()))).expect("ref")
}

fn policy() -> PolicyEnvelope {
    PolicyEnvelope {
        tenant_ref: opaque("tenant", "t"),
        access_policy_ref: opaque("policy", "a"),
        classification: Classification::Internal,
        retention_policy_ref: opaque("retention", "r"),
        deletion_policy_ref: opaque("deletion", "d"),
        legal_hold_ref: None,
        purpose_refs: vec![opaque("purpose", "p")],
    }
}

/// A request whose corpus carries a header and no examples: the server fills them.
fn score_request() -> OptimizationRequest {
    let program_ref = opaque("program", "program");
    let field = |name: &str, role| FieldSpec {
        name: name.to_string(),
        role,
        schema_ref: opaque("schema", name),
        description_ref: None,
        required: true,
    };
    OptimizationRequest {
        schema_version: PROGRAM_SCHEMA_VERSION,
        request_ref: opaque("optimization", "request"),
        program: ProgramRevision {
            schema_version: PROGRAM_SCHEMA_VERSION,
            program_ref: program_ref.clone(),
            revision: 1,
            parent_ref: None,
            signature: SignatureSpec {
                signature_ref: opaque("signature", "signature"),
                instruction_ref: opaque("instruction", "instruction"),
                fields: vec![
                    field("input", FieldRole::Input),
                    field("output", FieldRole::Output),
                ],
            },
            module: ModuleKind::Predict,
            adapter: AdapterKind::Chat,
            tool_refs: Vec::new(),
            policy: policy(),
        },
        corpus: TrainingCorpus {
            corpus_ref: opaque("corpus", "scores"),
            snapshot_version: 1,
            privacy: PrivacyAttestation {
                scanner_ref: opaque("scanner", "s"),
                policy_version_ref: opaque("privacy_policy", "pp"),
                raw_pii_persisted: false,
                local_identifiers_persisted: false,
            },
            examples: Vec::new(),
        },
        optimizer: OptimizerKind::LabeledFewShot,
        budget: OptimizationBudget {
            max_candidates: 8,
            max_demonstrations: ProgramModality::ALL.len(),
            max_model_calls: 0,
            max_evaluator_calls: 0,
            max_training_steps: 0,
            seed: 42,
        },
        promotion: PromotionPolicy::default(),
        baseline: EvaluationSummary {
            subject_ref: program_ref,
            aggregate_score: 0.5,
            modality_scores: [(ProgramModality::Graph, 0.5)].into_iter().collect(),
            evidence_refs: vec![opaque("evaluation", "baseline-evidence")],
        },
        optimizer_artifacts: Vec::new(),
        candidate_evaluations: Vec::new(),
        score_source: Some(ScoreSource {
            graph: GRAPH.to_string(),
        }),
    }
}

fn submit(id: u64, request: &OptimizationRequest) -> medulla_protocol_types::protocol::Request {
    test_support::request(
        SECRET,
        id,
        GRAPH,
        Method::AnalyticsJob {
            op: JobOp::Submit(SubmitJobSpec {
                graph: GRAPH.to_string(),
                tenant: String::new(),
                actor: String::new(),
                purpose: "program-score-source".to_string(),
                priority: 0,
                deadline_unix_ms: None,
                quota_cpu_ms: None,
                memory_bytes: None,
                io_bytes: None,
                output_bytes: None,
                worker_pool: String::new(),
                worker_region: String::new(),
                required_capabilities: Vec::new(),
                max_attempts: 1,
                backoff_ms: 0,
                kind: JobKind::ProgramOptimize {
                    request_msgpack: rmp_serde::to_vec_named(request).expect("encode request"),
                },
            }),
        },
    )
}

async fn put_node(
    state: &test_support::SharedState,
    id: u64,
    node: &str,
    props: serde_json::Value,
) {
    let response = Box::pin(test_support::dispatch(
        state,
        test_support::request(
            SECRET,
            id,
            GRAPH,
            Method::AddNode {
                node_id: node.to_string(),
                properties_msgpack: rmp_serde::to_vec_named(&props).expect("encode node"),
            },
        ),
    ))
    .await;
    assert!(
        response.error.is_none(),
        "seed node failed: {:?}",
        response.error
    );
}

async fn link_run(state: &test_support::SharedState, id: u64, score: &str, run: &str) {
    let response = Box::pin(test_support::dispatch(
        state,
        test_support::request(
            SECRET,
            id,
            GRAPH,
            Method::AddEdge {
                source_id: score.to_string(),
                target_id: run.to_string(),
                properties_msgpack: rmp_serde::to_vec_named(
                    &serde_json::json!({"relationship": "scoresRun"}),
                )
                .expect("encode edge"),
            },
        ),
    ))
    .await;
    assert!(
        response.error.is_none(),
        "seed edge failed: {:?}",
        response.error
    );
}

fn refusal(response: &Response) -> String {
    response.refusal_text().unwrap_or_default()
}

// spec: MEDULLA-DECISION-ENGINE-R813.2
#[test]
fn optimize_submit_builds_its_corpus_from_stored_scores() {
    let driver = medulla_server::spawn_engine_driver(|| {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_stack_size(medulla_server::ENGINE_WORKER_STACK_BYTES)
            .enable_all()
            .build()
            .expect("build runtime");
        runtime.block_on(async {
            let directory = tempfile::tempdir().expect("private persistence");
            let dir = directory.path().to_string_lossy().into_owned();
            let persistence: Arc<dyn PersistenceBackend> =
                Arc::new(RedbBackend::open(dir.clone(), 64).expect("open persistence"));
            let state = test_support::state_with(
                SECRET,
                common::current_isolation(),
                Some(dir),
                Some(persistence),
            );

            // No stored score: the server builds an empty corpus and the
            // request is refused; nothing the client sent stands in for it.
            let empty = Box::pin(test_support::dispatch(&state, submit(1, &score_request()))).await;
            assert!(
                empty.error.is_some(),
                "a graph with no scores must not stage a job"
            );

            for (n, (score, run, value, grader)) in [
                ("score-a", "run-a", 4.0, "human"),
                ("score-b", "run-b", 2.0, "model_judge"),
            ]
            .into_iter()
            .enumerate()
            {
                let base = 10 + (n as u64) * 4;
                put_node(
                    &state,
                    base,
                    run,
                    serde_json::json!({"type": "ExperimentRun"}),
                )
                .await;
                put_node(
                    &state,
                    base + 1,
                    score,
                    serde_json::json!({
                        "type": "Score", "value": value, "range_min": 1.0,
                        "range_max": 5.0, "grader_kind": grader,
                    }),
                )
                .await;
                link_run(&state, base + 2, score, run).await;
            }
            // A score with no run link is skipped, not fatal.
            put_node(
                &state,
                30,
                "score-orphan",
                serde_json::json!({
                    "type": "Score", "value": 3.0, "range_min": 1.0,
                    "range_max": 5.0, "grader_kind": "api",
                }),
            )
            .await;

            let staged =
                Box::pin(test_support::dispatch(&state, submit(40, &score_request()))).await;
            assert!(
                staged.error.is_none(),
                "the stored scores must build a corpus: {}",
                refusal(&staged)
            );
        });
    })
    .expect("start engine driver");
    medulla_server::join_engine_driver(driver).expect("engine driver failed");
}
