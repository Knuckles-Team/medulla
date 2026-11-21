//! MEDULLA-CQ-ORCH-R703.3: the served `ProcessModelDeploy` through the real `dispatch` shell
//! over a redb-backed `ServerState`.
//!
//! A deploy exports the stored model through the document export and queues one outbound
//! write-back proposal bound to the exported document's digest. It never calls the source and
//! never writes the graph. Process model sync is the served `SourceSync` with the process
//! adapter (MEDULLA-CQ-ORCH-R703.2); it has no method of its own.

#![cfg(feature = "redb")]

mod common;
#[path = "common/test_support.rs"]
mod test_support;

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
const CONNECTION: &str = "urn:connection:process-engine";

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
    Method::ProcessModelDeploy {
        model_iri: model_iri.into(),
        connection_iri: CONNECTION.into(),
    }
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
    let _imported: ProcessDocumentImported = payload(
        send(
            &state,
            2,
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
