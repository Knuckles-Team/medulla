//! MEDULLA-CQ-ORCH-R703.3: the served `ProcessModelDeploy` through the real `dispatch` shell
//! over a redb-backed `ServerState`.
//!
//! A deploy exports the stored model through the document export and queues one outbound
//! write-back proposal bound to the exported document's digest. It never calls the source and
//! never writes the graph. Process model sync is the served `SourceSync` with the process
//! adapter (MEDULLA-CQ-ORCH-R703.2); it has no method of its own.

#![cfg(all(feature = "redb", feature = "rdf", feature = "federation"))]

mod common;
#[path = "common/test_support.rs"]
mod test_support;

use std::collections::BTreeMap;
use std::sync::Arc;

use medulla_graph_core::graph::{GraphSchemaSource, GraphSchemaSources, SchemaSourceOrigin};
use medulla_protocol_types::protocol::{Method, Response, ResultPayload};
use medulla_source_types::source_sync::SyncReport;
use medulla_types::contract::Digest256;
use medulla_types::process_exchange::{
    ExportSource, ProcessDocument, ProcessDocumentExported, ProcessDocumentFormat,
    ProcessDocumentImported, PROCESS_MODEL_CLASS_IRI,
};

const SECRET: &str = "process-sync-served-secret";
const ENCRYPTION_KEY: &str = "process-sync-served-key-0123456789abcd";
const GRAPH: &str = "processes";
const MODEL_IRI: &str = "urn:model:every-kind";
const CONNECTION: &str = "urn:eg:fixture:process-sync:process-engine";
const OTHER_CONNECTION: &str = "urn:eg:fixture:process-sync:other-engine";
const AUTHORITY: &str =
    include_str!("../crates/medulla-core/tests/fixtures/process_sync/authority.ttl");

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

fn deploy(model_iri: &str) -> Method {
    deploy_to(model_iri, CONNECTION)
}

fn deploy_to(model_iri: &str, connection_iri: &str) -> Method {
    Method::ProcessModelDeploy {
        model_iri: model_iri.into(),
        connection_iri: connection_iri.into(),
    }
}

/// Attach the process engine authority rows of MEDULLA-CQ-ORCH-R703.1 to the graph.
fn attach_authority(state: &test_support::SharedState) {
    let guard = state.try_read().unwrap();
    let core = guard.registry.get(GRAPH).unwrap().core.clone();
    let mut sources = GraphSchemaSources::with_dynamic(BTreeMap::new()).unwrap();
    let source = GraphSchemaSource::new(
        SchemaSourceOrigin::Operator,
        None,
        Some(Arc::from(AUTHORITY)),
        0,
    )
    .expect("a bounded operator source");
    sources
        .attach_dynamic("operator".to_string(), source)
        .expect("the operator source attaches");
    core.install_schema_sources(Arc::new(sources));
}

async fn import_model(state: &test_support::SharedState, id: u64) {
    let _imported: ProcessDocumentImported = payload(
        send(
            state,
            id,
            Method::ProcessDocumentImport {
                document: ProcessDocument {
                    schema_version: 1,
                    format: ProcessDocumentFormat::Bpmn20,
                    document: EVERY_KIND.to_vec(),
                    source_ref: None,
                },
                model_iri: MODEL_IRI.into(),
                expected_version: Some(0),
            },
        )
        .await,
    );
}

// spec: MEDULLA-CQ-ORCH-R703.3
#[tokio::test(flavor = "multi_thread")]
async fn deploy_queues_one_proposal_with_the_document_digest() {
    let (_dir, _backend, state) = test_support::durable_graph_state(
        SECRET,
        ENCRYPTION_KEY,
        "process-sync-served",
        common::current_isolation(),
        GRAPH,
    )
    .await;
    attach_authority(&state);
    import_model(&state, 2).await;
    let exported: ProcessDocumentExported = payload(
        send(
            &state,
            3,
            Method::ProcessDocumentExport {
                source: ExportSource::Model {
                    model_iri: MODEL_IRI.into(),
                    version: None,
                },
                format: ProcessDocumentFormat::Bpmn20,
            },
        )
        .await,
    );

    let before = core_version(&state);
    let report: SyncReport = payload(send(&state, 4, deploy(MODEL_IRI)).await);
    assert_eq!(report.connection_iri, CONNECTION);
    assert_eq!(report.class_iri, PROCESS_MODEL_CLASS_IRI);
    assert_eq!(report.proposed, 1);
    assert_eq!(report.outbound.len(), 1, "one model, one proposal");
    assert_eq!(report.outbound[0].record_key, MODEL_IRI);
    assert_eq!(
        report.outbound[0].payload_digest, exported.document_digest,
        "the proposal names the exported document"
    );

    // The queue id is the digest of the connection, the class and the document digest.
    let mut framed = Vec::new();
    for part in [CONNECTION, PROCESS_MODEL_CLASS_IRI, exported.document_digest.as_str()] {
        framed.extend_from_slice(part.as_bytes());
        framed.push(0);
    }
    let queue_id = Digest256::sha256(&framed).to_hex();
    assert_eq!(report.queue_id.as_deref(), Some(queue_id.as_str()));
    assert_eq!(
        report.queued,
        vec![format!("sync:{}:{MODEL_IRI}", &queue_id[..16])]
    );
    assert_eq!(core_version(&state), before, "a deploy never writes the graph");

    // A second deploy of the unchanged model queues the same proposal: no duplicate.
    let again: SyncReport = payload(send(&state, 5, deploy(MODEL_IRI)).await);
    assert_eq!(again.queue_id, report.queue_id);
    assert_eq!(again.queued, report.queued);
}

// spec: MEDULLA-CQ-ORCH-R703.4
#[tokio::test(flavor = "multi_thread")]
async fn sync_refusals_queue_nothing() {
    let (_dir, _backend, state) = test_support::durable_graph_state(
        SECRET,
        ENCRYPTION_KEY,
        "process-sync-refusals",
        common::current_isolation(),
        GRAPH,
    )
    .await;
    attach_authority(&state);
    import_model(&state, 2).await;
    let before = core_version(&state);

    // A connection the authority rows do not declare a federated secondary.
    let foreign = send(&state, 3, deploy_to(MODEL_IRI, OTHER_CONNECTION)).await;
    assert!(
        foreign
            .error
            .as_deref()
            .is_some_and(|error| error.starts_with("PROCESS_SYNC_SOURCE_NOT_SECONDARY")),
        "got {:?}",
        foreign.error
    );

    // A model the caller cannot see is refused with the export's code.
    let unknown = send(&state, 4, deploy("urn:model:missing")).await;
    assert!(unknown
        .error
        .as_deref()
        .is_some_and(|error| error.starts_with("PROCESS_IMPORT_UNKNOWN_MODEL")));
    assert_eq!(core_version(&state), before, "a refusal writes nothing");

    // Nothing was queued: the first accepted deploy still reports a fresh queue.
    let accepted: SyncReport = payload(send(&state, 5, deploy(MODEL_IRI)).await);
    assert_eq!(accepted.queued.len(), 1);
}
