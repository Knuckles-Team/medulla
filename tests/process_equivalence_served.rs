//! MEDULLA-CQ-ORCH-R701.9: one process authored as BPMN 2.0 and as an EPC canonical form
//! gives one canonical workflow digest and one plan digest through the real `dispatch` shell
//! over a redb-backed `ServerState`.
//!
//! The BPMN side enters through `ProcessDocumentImport` then `ProcessCompile`; the EPC side
//! enters as the canonical form the ARIS adapter produces, through `ProcessModelImport`. Both
//! stored workflows are then planned through `Decide` with `QuestionKind::ProcessPlan`.

#![cfg(all(feature = "redb", feature = "decide"))]

mod common;
#[path = "common/test_support.rs"]
mod test_support;

use medulla_agent_types::agent_component::AgentComponentKind;
use medulla_decision_types::decision::statistical::body::encode_body;
use medulla_decision_types::decision::statistical::declared::DeclaredOption;
use medulla_decision_types::decision::statistical::features::{
    FeatureKind, FeatureSchemaBody, FeatureSpec, MissingValue, FEATURE_SCHEMA_VERSION,
};
use medulla_decision_types::decision::statistical::{
    CandidateSource, DecideRequest, DecisionBatch, QuestionKind, QuestionSafety,
    StatisticalQuestion, TypedParam, TypedValue,
};
use medulla_decision_types::decision::DecisionPolicyRef;
use medulla_protocol_types::protocol::{Method, Response, ResultPayload};
use medulla_types::contract::BoundedVec;
use medulla_types::process_exchange::{ProcessDocument, ProcessDocumentFormat};
use medulla_types::process_import::{CanonicalWorkflow, ProcessCompiled, ProcessModelImported};

const SECRET: &str = "process-equivalence-served-secret";
const ENCRYPTION_KEY: &str = "process-equivalence-served-key-0123456789";
const GRAPH: &str = "processes";
const MODEL_IRI: &str = "urn:model:invoice-approval";
const EPC_WORKFLOW_ID: &str = "invoice-approval-epc";

const BPMN: &[u8] = include_bytes!(
    "../crates/medulla-core/tests/fixtures/process_equivalence/invoice-approval.bpmn"
);
const GOLDEN: &str = include_str!(
    "../crates/medulla-core/tests/fixtures/process_equivalence/invoice-approval.canonical.json"
);
const EPC_SIDE: &str = include_str!(
    "../crates/medulla-core/tests/fixtures/process_equivalence/invoice-approval.epc.canonical.json"
);

async fn send(state: &test_support::SharedState, id: u64, method: Method) -> Response {
    test_support::dispatch(state, test_support::request(SECRET, id, GRAPH, method)).await
}

fn payload<T: serde::de::DeserializeOwned>(response: Response) -> T {
    assert!(response.error.is_none(), "refused: {:?}", response.error);
    match response.result {
        Some(ResultPayload::Json(value)) => serde_json::from_value(value).expect("typed answer"),
        Some(ResultPayload::Raw(bytes)) => rmp_serde::from_slice(&bytes).expect("typed answer"),
        other => panic!("unexpected payload {other:?}"),
    }
}

async fn feature_schema(
    state: &test_support::SharedState,
) -> medulla_agent_types::agent_component::ComponentDependency {
    use medulla_h_decide::decide::stat_test_support::Harness;
    use medulla_persistence_records::agent_component::test_component_draft;
    use medulla_persistence_records::agent_library::AgentLibraryStateExt;

    let store = state.write().await.ensure_agent_library().unwrap();
    let harness = Harness {
        _dir: tempfile::tempdir().unwrap(),
        state: state.clone(),
        store: store.clone(),
        nonce: std::cell::Cell::new(1),
    };
    let body = FeatureSchemaBody {
        schema_version: FEATURE_SCHEMA_VERSION,
        features: BoundedVec::new(vec![FeatureSpec {
            name: "score".to_string(),
            kind: FeatureKind::Number {
                key: "score".to_string(),
            },
            missing: MissingValue::Abstain,
        }])
        .unwrap(),
    };
    let encoded = encode_body(&body).unwrap();
    let mut draft = test_component_draft(common::TEST_TENANT, "schema-process-equivalence");
    draft.kind = AgentComponentKind::FeatureSchema;
    draft.summary = "process plan features".to_string();
    draft.content_digest = encoded.content_digest;
    draft.attributes = encoded.attributes;
    harness.commit_in(common::TEST_TENANT, draft, None).unwrap()
}

fn plan_request(
    schema: &medulla_agent_types::agent_component::ComponentDependency,
    workflow_id: &str,
    version: u32,
) -> DecideRequest {
    let body = serde_json::json!({ "workflow_id": workflow_id, "version": version }).to_string();
    DecideRequest {
        tenant_id: common::TEST_TENANT.to_string(),
        question: StatisticalQuestion {
            question_id: format!("equivalence.{workflow_id}"),
            kind: QuestionKind::ProcessPlan,
            safety: QuestionSafety::Ordinary,
        },
        candidates: CandidateSource::Declared {
            options: BoundedVec::new(vec![DeclaredOption {
                option_id: "process_plan".to_string(),
                classification: BoundedVec::default(),
                numbers: BoundedVec::default(),
                texts: BoundedVec::default(),
            }])
            .unwrap(),
        },
        feature_schema: schema.clone(),
        head: None,
        policy: DecisionPolicyRef::Default,
        params: BoundedVec::new(vec![
            TypedParam {
                name: "graph".to_string(),
                value: TypedValue::Text(GRAPH.to_string()),
            },
            TypedParam {
                name: "process_plan".to_string(),
                value: TypedValue::Text(body),
            },
        ])
        .unwrap(),
        max_records: None,
        belief_as_of: BoundedVec::default(),
        evidence_streams: Default::default(),
    }
}

/// The task kinds and edges of a plan: `(task id, kind, sorted dependencies)` per task.
type Shape = Vec<(String, &'static str, Vec<String>)>;

fn shape(plan: &medulla_decision_types::decision::task_plan::TaskPlan) -> Shape {
    plan.tasks
        .as_slice()
        .iter()
        .map(|task| {
            let kind = if task.call.is_some() {
                "call"
            } else if task.branch.is_some() {
                "branch"
            } else if task.join.is_some() {
                "join"
            } else if task.decision.is_some() {
                "decision"
            } else if task.approval.is_some() {
                "approval"
            } else {
                "prompt"
            };
            let mut deps: Vec<String> = task.depends_on.as_slice().to_vec();
            deps.sort();
            (task.task_id.clone(), kind, deps)
        })
        .collect()
}

/// The id of the BPMN process element, read from the imported document itself so the test
/// never hard-codes the node id the importer derives from it.
fn bpmn_process_element_id() -> String {
    let text = std::str::from_utf8(BPMN).expect("the BPMN fixture is UTF-8");
    let start = text.find("<process ").expect("a process element");
    let end = start + text[start..].find('>').expect("the end of the process tag");
    let after = text[start..end]
        .split("id=\"")
        .nth(1)
        .expect("a process id attribute");
    after.split('"').next().expect("a closed id").to_string()
}

async fn plan_digest(
    state: &test_support::SharedState,
    id: u64,
    schema: &medulla_agent_types::agent_component::ComponentDependency,
    workflow_id: &str,
    version: u32,
) -> (String, Shape) {
    let batch: DecisionBatch = payload(
        send(
            state,
            id,
            Method::Decide {
                request: Box::new(plan_request(schema, workflow_id, version)),
            },
        )
        .await,
    );
    let plan = batch.records.as_slice()[0]
        .task_plan()
        .expect("the sealed record holds the plan");
    (plan.plan_digest.clone(), shape(plan))
}

// spec: MEDULLA-CQ-ORCH-R701.9
#[tokio::test(flavor = "multi_thread")]
async fn bpmn_and_epc_give_one_workflow_and_one_plan() {
    let (_dir, _backend, state) = test_support::durable_graph_state(
        SECRET,
        ENCRYPTION_KEY,
        "process-equivalence-served",
        common::current_isolation(),
        GRAPH,
    )
    .await;
    let golden: CanonicalWorkflow = serde_json::from_str(GOLDEN).expect("the golden workflow");

    // BPMN: import the document, then compile the process it wrote.
    let imported = send(
        &state,
        1,
        Method::ProcessDocumentImport {
            document: ProcessDocument {
                schema_version: 1,
                format: ProcessDocumentFormat::Bpmn20,
                document: BPMN.to_vec(),
                source_ref: None,
            },
            model_iri: MODEL_IRI.into(),
            expected_version: Some(0),
        },
    )
    .await;
    assert!(imported.error.is_none(), "refused: {:?}", imported.error);
    let process_iri = format!("process:{MODEL_IRI}:{}", bpmn_process_element_id());
    let bpmn: ProcessCompiled = payload(
        send(
            &state,
            2,
            Method::ProcessCompile {
                process_iri: process_iri.clone(),
                expected_version: Some(0),
                preconditions: Vec::new(),
            },
        )
        .await,
    );

    // EPC: the canonical form the adapter recorded.
    let epc_workflow: CanonicalWorkflow = serde_json::from_str(EPC_SIDE).expect("the EPC side");
    let epc: ProcessModelImported = payload(
        send(
            &state,
            3,
            Method::ProcessModelImport {
                workflow: epc_workflow.clone(),
                workflow_id: EPC_WORKFLOW_ID.into(),
                expected_version: Some(0),
                preconditions: Vec::new(),
            },
        )
        .await,
    );

    assert_eq!(
        bpmn.workflow_digest,
        golden.digest.clone(),
        "the BPMN side equals the golden"
    );
    assert_eq!(
        epc.workflow_digest,
        golden.digest.clone(),
        "one process, one workflow digest"
    );
    assert_eq!(
        golden.steps.len(),
        12,
        "the invoice process has 12 canonical steps"
    );
    assert_ne!(
        serde_json::to_value(&epc_workflow.annex).unwrap(),
        serde_json::json!([]),
        "the EPC side keeps an annex that sits outside the digest"
    );

    // One plan digest for both workflows.
    let schema = feature_schema(&state).await;
    let (bpmn_plan, bpmn_shape) = plan_digest(&state, 4, &schema, &process_iri, bpmn.version).await;
    let (epc_plan, epc_shape) = plan_digest(&state, 5, &schema, EPC_WORKFLOW_ID, epc.version).await;
    assert_eq!(bpmn_plan, epc_plan, "one process, one plan digest");
    assert_eq!(bpmn_shape, epc_shape, "the same task kinds and edges");
}
