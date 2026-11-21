//! MEDULLA-CQ-ORCH-R702.9: the served `ProcessDocumentImport` through the real `dispatch`
//! shell over a redb-backed `ServerState`.
//!
//! An import reads one BPMN 2.0 or DMN 1.3 document natively, maps it onto element nodes and
//! commits them with the model node as one batch under a compare-and-set on the model version.

#![cfg(feature = "redb")]

mod common;
#[path = "common/test_support.rs"]
mod test_support;

use medulla_protocol_types::protocol::{Method, Response, ResultPayload};
use medulla_types::process_exchange::{
    ProcessDocument, ProcessDocumentFormat, ProcessDocumentImported,
};

const SECRET: &str = "process-exchange-served-secret";
const ENCRYPTION_KEY: &str = "process-exchange-served-key-0123456789a";
const GRAPH: &str = "processes";
const MODEL_IRI: &str = "urn:model:every-kind";

const EVERY_KIND: &[u8] =
    include_bytes!("../crates/medulla-core/tests/fixtures/process_exchange/every_kind.bpmn");

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

fn import_method(bytes: &[u8], expected_version: Option<u32>) -> Method {
    Method::ProcessDocumentImport {
        document: ProcessDocument {
            schema_version: 1,
            format: ProcessDocumentFormat::Bpmn20,
            document: bytes.to_vec(),
            source_ref: None,
        },
        model_iri: MODEL_IRI.into(),
        expected_version,
    }
}

fn node_type(state: &test_support::SharedState, id: &str) -> Option<String> {
    let guard = state.try_read().unwrap();
    let core = &guard.registry.get(GRAPH).unwrap().core;
    let bytes = core.get_node_properties(id)?;
    let properties = medulla_types::msgpack::decode_property_object(&bytes).ok()?;
    properties
        .get("type")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
}

// spec: MEDULLA-CQ-ORCH-R702.9
#[tokio::test(flavor = "multi_thread")]
async fn import_writes_model_in_one_batch() {
    let (_dir, _backend, state) = test_support::durable_graph_state(
        SECRET,
        ENCRYPTION_KEY,
        "process-exchange-served",
        common::current_isolation(),
        GRAPH,
    )
    .await;

    let before = core_version(&state);
    let first: ProcessDocumentImported =
        payload(send(&state, 2, import_method(EVERY_KIND, Some(0))).await);
    assert_eq!(first.version, 1);
    assert_eq!(first.model_node_id, format!("processmodel:{MODEL_IRI}:v1"));
    assert!(first.nodes > 0 && first.edges > 0);
    assert_eq!(first.source_document_digest.len(), 64);
    assert_eq!(core_version(&state), before + 1, "one import commits one batch");

    // The stored classes: the model node and the user task of the document.
    assert_eq!(
        node_type(&state, &first.model_node_id).as_deref(),
        Some("ProcessModel")
    );
    assert_eq!(
        node_type(&state, &format!("process:{MODEL_IRI}:user1")).as_deref(),
        Some("BpmnUserTask")
    );

    // The same document under the stored version is version 2.
    let second: ProcessDocumentImported =
        payload(send(&state, 3, import_method(EVERY_KIND, Some(1))).await);
    assert_eq!(second.version, 2);

    // A stale version and a document the reader refuses write nothing.
    let version = core_version(&state);
    let stale = send(&state, 4, import_method(EVERY_KIND, Some(1))).await;
    assert!(stale
        .error
        .as_deref()
        .is_some_and(|error| error.starts_with("WORKFLOW_VERSION_CONFLICT")));
    let refused = send(&state, 5, import_method(b"<not-bpmn", None)).await;
    assert!(refused.error.is_some());
    assert_eq!(core_version(&state), version, "a refusal writes nothing");
}
