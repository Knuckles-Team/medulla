//! MEDULLA-CQ-ORCH-R701.3: the served `ProcessModelImport` and `ProcessModelExport` through
//! the real `dispatch` shell over a redb-backed `ServerState`.
//!
//! An import admits one canonical workflow and stores it as the next `WorkflowPattern`
//! version under a compare-and-set; an export reads one stored version back as the canonical
//! form, as a deployable BPMN document, or with the losses of the EPC target.

#![cfg(feature = "redb")]

mod common;
#[path = "common/test_support.rs"]
mod test_support;

use medulla_protocol_types::protocol::{Method, Response, ResultPayload};
use medulla_types::process_import::{
    CanonicalWorkflow, ProcessModelExported, ProcessModelImported, ProcessSourceSystem,
};

const SECRET: &str = "process-import-served-secret";
const ENCRYPTION_KEY: &str = "process-import-served-key-0123456789abc";
const GRAPH: &str = "processes";
const WORKFLOW_ID: &str = "invoice-approval";

const BPMN_SIDE: &str = include_str!(
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

fn core_version(state: &test_support::SharedState) -> u64 {
    state
        .try_read()
        .unwrap()
        .registry
        .get(GRAPH)
        .unwrap()
        .core
        .version()
}

fn workflow(text: &str) -> CanonicalWorkflow {
    serde_json::from_str(text).expect("a canonical fixture")
}

async fn import(
    state: &test_support::SharedState,
    id: u64,
    text: &str,
    expected_version: Option<u32>,
) -> Response {
    send(
        state,
        id,
        Method::ProcessModelImport {
            workflow: workflow(text),
            workflow_id: WORKFLOW_ID.into(),
            expected_version,
            preconditions: Vec::new(),
        },
    )
    .await
}

async fn export(
    state: &test_support::SharedState,
    id: u64,
    version: u32,
    target: Option<ProcessSourceSystem>,
    strict: bool,
) -> Response {
    send(
        state,
        id,
        Method::ProcessModelExport {
            workflow_id: WORKFLOW_ID.into(),
            version,
            target,
            strict,
        },
    )
    .await
}

// spec: MEDULLA-CQ-ORCH-R701.3
#[tokio::test(flavor = "multi_thread")]
async fn import_stores_versioned_workflow_and_export_reads_it() {
    let (_dir, _backend, state) = test_support::durable_graph_state(
        SECRET,
        ENCRYPTION_KEY,
        "process-import-served",
        common::current_isolation(),
        GRAPH,
    )
    .await;
    let stated = workflow(BPMN_SIDE);

    let before = core_version(&state);
    let first: ProcessModelImported = payload(import(&state, 2, BPMN_SIDE, Some(0)).await);
    assert_eq!(first.version, 1);
    assert_eq!(first.workflow_id, "workflow:invoice-approval:v1");
    assert_eq!(first.workflow_digest, stated.digest);
    assert_eq!(
        core_version(&state),
        before + 1,
        "one import commits one batch"
    );

    // The EPC side of the same process carries the same digest and enters as version 2.
    let second: ProcessModelImported = payload(import(&state, 3, EPC_SIDE, Some(1)).await);
    assert_eq!(second.version, 2);
    assert_eq!(second.workflow_digest, stated.digest);

    let version = core_version(&state);
    let stale = import(&state, 4, BPMN_SIDE, Some(1)).await;
    assert_eq!(stale.error.as_deref(), Some("WORKFLOW_VERSION_CONFLICT"));
    let mut tampered = workflow(BPMN_SIDE);
    tampered.digest = "0".repeat(64);
    let refused = send(
        &state,
        5,
        Method::ProcessModelImport {
            workflow: tampered,
            workflow_id: WORKFLOW_ID.into(),
            expected_version: None,
            preconditions: Vec::new(),
        },
    )
    .await;
    assert_eq!(
        refused.error.as_deref(),
        Some("PROCESS_IMPORT_DIGEST_MISMATCH")
    );
    assert_eq!(core_version(&state), version, "a refusal writes nothing");

    // No target: the canonical form as stored, version 1.
    let canonical: ProcessModelExported = payload(export(&state, 6, 1, None, false).await);
    assert_eq!(canonical.workflow, stated);
    assert!(canonical.document.is_none());
    assert_eq!(core_version(&state), version, "an export commits nothing");

    // `bpmn20`: the canonical form and a deployable BPMN document.
    let bpmn: ProcessModelExported =
        payload(export(&state, 7, 1, Some(ProcessSourceSystem::Bpmn20), false).await);
    assert_eq!(bpmn.workflow.digest, stated.digest);
    assert!(bpmn.document.is_some_and(|text| text.contains("process")));

    // `aris_epc`: the canonical form with the EPC losses; the digest is kept.
    let epc: ProcessModelExported =
        payload(export(&state, 8, 1, Some(ProcessSourceSystem::ArisEpc), false).await);
    assert_eq!(epc.workflow.digest, stated.digest);

    // The EPC-sourced version loses its annex toward BPMN: strict refuses, lenient lists it.
    let lossy = export(&state, 9, 2, Some(ProcessSourceSystem::Bpmn20), true).await;
    assert_eq!(lossy.error.as_deref(), Some("PROCESS_EXPORT_LOSSY"));
    let listed: ProcessModelExported =
        payload(export(&state, 10, 2, Some(ProcessSourceSystem::Bpmn20), false).await);
    assert!(!listed.workflow.losses.is_empty());

    let unknown = export(&state, 11, 9, None, false).await;
    assert_eq!(
        unknown.error.as_deref(),
        Some("PROCESS_IMPORT_UNKNOWN_MODEL")
    );
}

fn node_properties(
    state: &test_support::SharedState,
    id: &str,
) -> serde_json::Map<String, serde_json::Value> {
    let guard = state.try_read().unwrap();
    let core = &guard.registry.get(GRAPH).unwrap().core;
    let bytes = core
        .get_nodes()
        .into_iter()
        .find(|(node, _)| node == id)
        .unwrap_or_else(|| panic!("node {id} is stored"))
        .1;
    medulla_types::msgpack::decode_property_object(&bytes).expect("node properties")
}

// spec: MEDULLA-CQ-ORCH-R701.5
#[tokio::test(flavor = "multi_thread")]
async fn stored_steps_keep_source_ids() {
    let (_dir, _backend, state) = test_support::durable_graph_state(
        SECRET,
        ENCRYPTION_KEY,
        "process-import-evidence",
        common::current_isolation(),
        GRAPH,
    )
    .await;
    let stated = workflow(EPC_SIDE);
    let imported: ProcessModelImported = payload(import(&state, 2, EPC_SIDE, Some(0)).await);

    let node = node_properties(&state, &imported.workflow_id);
    assert_eq!(node["workflowDigest"], stated.digest);
    assert_eq!(node["importedFrom"], stated.source.model_id);
    let annex: serde_json::Value =
        serde_json::from_str(node["processAnnex"].as_str().unwrap()).unwrap();
    assert_eq!(annex, serde_json::to_value(&stated.annex).unwrap());
    assert!(!stated.annex.is_empty(), "the EPC side keeps an annex");

    for step in &stated.steps {
        let stored = node_properties(
            &state,
            &format!("{}:step:{}", imported.workflow_id, step.id),
        );
        assert_eq!(stored["sourceSystem"], "aris_epc");
        assert_eq!(stored["sourceModelId"], stated.source.model_id);
        assert_eq!(
            stored["sourceElementId"].as_str(),
            stated.source.elements.get(&step.id).map(String::as_str)
        );
    }
}
