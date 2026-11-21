//! The served program-optimization journey shared by the program job L3 tests
//! (MEDULLA-DECISION-ENGINE-R810.3, MEDULLA-DECISION-ENGINE-R810.7).
//!
//! The colocated job executor is one process-wide pool bound to the first state
//! that submits a job, so each test binary that includes this module runs
//! exactly one journey.
#![allow(dead_code)]

use std::sync::Arc;
use std::time::Duration;

use medulla_modality::{
    ArtifactId, Classification, DerivationId, EvidenceAddress, EvidenceLocus, EvidenceLocusId,
    OpaqueRef, PolicyEnvelope, PrivacyAttestation, ResourceId,
};
use medulla_persistence::redb_backend::RedbBackend;
use medulla_program::{
    AdapterKind, EvaluationSummary, EvidenceBinding, ExampleOutcome, ExampleSplit, FieldRole,
    FieldSpec, GraderKind, ModuleKind, OptimizationBudget, OptimizationRequest, OptimizerKind,
    ProgramModality, ProgramRevision, ProgramRevisionIdentity, PromotionPolicy, SignatureSpec,
    TrainingCorpus, TrainingExample, PROGRAM_SCHEMA_VERSION,
};
use medulla_protocol_types::protocol::{Method, Request, Response, ResultPayload};
use medulla_server_api::api::persistence::PersistenceBackend;
use medulla_work_types::{jobs::JobKind, jobs::JobOp, jobs::SubmitJobSpec};
use sha2::{Digest, Sha256};

pub const SECRET: &str = "program-job-served-secret";
pub const GRAPH: &str = "__commons__";
/// Test-split examples of the fixture corpus.
pub const TEST_EXAMPLES: u64 = 2;

fn token(label: &str) -> String {
    hex::encode(Sha256::digest(label.as_bytes()))
}

fn opaque(namespace: &str, label: &str) -> OpaqueRef {
    OpaqueRef::scoped(namespace, &token(label)).expect("test opaque reference")
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

/// One graph-modality example: six train examples and `TEST_EXAMPLES` test examples.
fn example(index: usize, policy: &PolicyEnvelope) -> TrainingExample {
    let artifact = ArtifactId::from_token(&token(&format!("artifact:{index}"))).expect("artifact");
    TrainingExample {
        example_ref: opaque("example", &format!("example:{index}")),
        input_refs: vec![opaque("content", &format!("input:{index}"))],
        expected_output_ref: Some(opaque("content", &format!("expected:{index}"))),
        observed_output_ref: Some(opaque("content", &format!("observed:{index}"))),
        feedback_ref: Some(opaque("feedback", &format!("feedback:{index}"))),
        trace_ref: Some(opaque("trace", &format!("example-trace:{index}"))),
        split: if index < 6 {
            ExampleSplit::Train
        } else {
            ExampleSplit::Test
        },
        grader_kind: GraderKind::Annotation,
        score_ref: None,
        author: None,
        outcome: ExampleOutcome::Success,
        score: 0.7 + (index as f64 / 100.0),
        weight: 1.0,
        evidence: vec![EvidenceBinding {
            modality: ProgramModality::Graph,
            locus: EvidenceLocus {
                id: EvidenceLocusId::from_token(&token(&format!("locus:{index}"))).expect("locus"),
                subject: ResourceId::Artifact(artifact),
                address: EvidenceAddress::RowVersion {
                    row_ref: opaque("row", &format!("row:{index}")),
                    version: 1,
                },
                policy_ref: policy.access_policy_ref.clone(),
                derivation_ref: DerivationId::from_token(&token(&format!("derivation:{index}")))
                    .expect("derivation"),
            },
            policy: policy.clone(),
        }],
    }
}

/// A request over the fixture corpus. `evaluations` are the candidate evaluations
/// the caller read back from an earlier job's result.
pub fn program_request(label: &str, evaluations: Vec<EvaluationSummary>) -> OptimizationRequest {
    let policy = policy();
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
        request_ref: opaque("optimization", label),
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
            policy: policy.clone(),
        },
        corpus: TrainingCorpus {
            corpus_ref: opaque("corpus", "corpus"),
            snapshot_version: 7,
            privacy: PrivacyAttestation {
                scanner_ref: opaque("scanner", "s"),
                policy_version_ref: opaque("privacy_policy", "pp"),
                raw_pii_persisted: false,
                local_identifiers_persisted: false,
            },
            examples: (0..(6 + TEST_EXAMPLES as usize))
                .map(|index| example(index, &policy))
                .collect(),
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
        candidate_evaluations: evaluations,
        score_source: None,
    }
}

/// The program the fixture optimizes.
pub fn program_ref() -> OpaqueRef {
    opaque("program", "program")
}

pub fn evaluation_of(candidate_ref: &str, score: f64) -> EvaluationSummary {
    EvaluationSummary {
        subject_ref: OpaqueRef::new(candidate_ref.to_string()).expect("candidate ref"),
        aggregate_score: score,
        modality_scores: [(ProgramModality::Graph, score)].into_iter().collect(),
        evidence_refs: vec![opaque("evaluation", "candidate-evidence")],
    }
}

/// A server state over a private Redb persistence directory, the durable path
/// every job store and graph write of the journey uses.
pub fn redb_state(dir: &std::path::Path) -> crate::test_support::SharedState {
    let dir = dir.to_string_lossy().into_owned();
    let persistence: Arc<dyn PersistenceBackend> =
        Arc::new(RedbBackend::open(dir.clone(), 64).expect("open persistence"));
    crate::test_support::state_with(
        SECRET,
        crate::common::current_isolation(),
        Some(dir),
        Some(persistence),
    )
}

fn submit_request(id: u64, request: &OptimizationRequest) -> Request {
    crate::test_support::request(
        SECRET,
        id,
        GRAPH,
        Method::AnalyticsJob {
            op: JobOp::Submit(SubmitJobSpec {
                graph: GRAPH.to_string(),
                tenant: String::new(),
                actor: String::new(),
                purpose: "program-job-served".to_string(),
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

fn json_of(response: Response) -> serde_json::Value {
    assert!(
        response.error.is_none(),
        "dispatch failed: {:?}",
        response.error
    );
    match response.result {
        Some(ResultPayload::Json(value)) => value,
        other => panic!("expected JSON response, received {other:?}"),
    }
}

/// Submit one program job, wait until the colocated executor claims, runs and
/// publishes it, and return its status. `id` seeds this job's request ids.
pub async fn run_program_job(
    state: &crate::test_support::SharedState,
    id: u64,
    request: &OptimizationRequest,
) -> serde_json::Value {
    let submitted =
        json_of(Box::pin(crate::test_support::dispatch(state, submit_request(id, request))).await);
    let job_id = submitted
        .get("job_id")
        .and_then(serde_json::Value::as_str)
        .expect("submitted job id")
        .to_string();
    for attempt in 1..=400 {
        let status = json_of(
            Box::pin(crate::test_support::dispatch(
                state,
                crate::test_support::request(
                    SECRET,
                    id + attempt,
                    GRAPH,
                    Method::AnalyticsJob {
                        op: JobOp::Status {
                            job_id: job_id.clone(),
                        },
                    },
                ),
            ))
            .await,
        );
        let job_state = status.get("state").expect("job state");
        if job_state.get("Succeeded").is_some() {
            return status;
        }
        assert!(
            job_state.get("Failed").is_none() && job_state.get("Cancelled").is_none(),
            "the program job terminated unsuccessfully: {status}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("the program job did not publish before the timeout");
}

/// The first string under `value` that starts with `prefix`.
pub fn first_string_with_prefix(value: &serde_json::Value, prefix: &str) -> Option<String> {
    match value {
        serde_json::Value::String(text) if text.starts_with(prefix) => Some(text.clone()),
        serde_json::Value::Array(items) => items
            .iter()
            .find_map(|item| first_string_with_prefix(item, prefix)),
        serde_json::Value::Object(map) => map
            .values()
            .find_map(|item| first_string_with_prefix(item, prefix)),
        _ => None,
    }
}

pub async fn node_properties(
    state: &crate::test_support::SharedState,
    id: u64,
    node_id: &str,
) -> Option<serde_json::Value> {
    let response = Box::pin(crate::test_support::dispatch(
        state,
        crate::test_support::request(
            SECRET,
            id,
            GRAPH,
            Method::GetNodeProperties {
                node_id: node_id.to_string(),
            },
        ),
    ))
    .await;
    assert!(response.error.is_none(), "read refused: {response:?}");
    match response.result.as_ref() {
        Some(ResultPayload::Raw(bytes)) => {
            rmp_serde::from_slice::<Option<serde_json::Value>>(bytes).expect("decode node")
        }
        _ => None,
    }
}

/// What the journey leaves on the graph.
pub struct Journey {
    /// The revision node the promotion committed.
    pub revision: serde_json::Value,
    /// The id of that node.
    pub revision_ref: String,
    /// The evaluated job's id.
    pub job_id: String,
}

/// Two jobs over one corpus. The first has no evaluation and stays a draft; its
/// result names the candidate the server derived under the caller's policy. The
/// second carries that candidate's evaluation, reaches `Evaluated` and commits
/// the revision and the active pointer. Returns the committed revision node.
pub async fn evaluated_journey(state: &crate::test_support::SharedState) -> Journey {
    let draft = run_program_job(state, 100, &program_request("draft", Vec::new())).await;
    let candidate = first_string_with_prefix(&draft, "eg:program_candidate:")
        .expect("the draft result names its candidate");
    assert!(
        node_properties(
            state,
            200,
            ProgramRevisionIdentity::active_pointer_ref_for(&program_ref()).as_str()
        )
        .await
        .is_none(),
        "a draft moves no pointer"
    );

    let evaluated = program_request("evaluated", vec![evaluation_of(&candidate, 0.8)]);
    let status = run_program_job(state, 1_000, &evaluated).await;
    let job_id = status
        .get("job_id")
        .and_then(serde_json::Value::as_str)
        .expect("the status names its job")
        .to_string();
    let pointer = node_properties(
        state,
        2_000,
        ProgramRevisionIdentity::active_pointer_ref_for(&program_ref()).as_str(),
    )
    .await
    .expect("the evaluated job moved the active pointer");
    let revision_ref = pointer
        .get("revision_ref")
        .and_then(serde_json::Value::as_str)
        .expect("the pointer names its revision")
        .to_string();
    let revision = node_properties(state, 2_001, &revision_ref)
        .await
        .expect("the revision node is stored");
    Journey {
        revision,
        revision_ref,
        job_id,
    }
}

/// Read `job_id`'s status as a caller of another tenant.
pub async fn foreign_status(
    state: &crate::test_support::SharedState,
    id: u64,
    job_id: &str,
) -> Response {
    let mut context = crate::common::context_for(crate::common::TEST_AGENT);
    context.tenant = "tenant-foreign".to_string();
    Box::pin(crate::test_support::dispatch(
        state,
        crate::common::signed_request_with_context(
            SECRET,
            id,
            GRAPH,
            context,
            Method::AnalyticsJob {
                op: JobOp::Status {
                    job_id: job_id.to_string(),
                },
            },
        ),
    ))
    .await
}
