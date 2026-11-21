//! MEDULLA-CQ-ORCH-R702.9: the served `ProcessDocumentImport` through the real `dispatch`
//! shell over a redb-backed `ServerState`.
//!
//! An import reads one BPMN 2.0 or DMN 1.3 document natively, maps it onto element nodes and
//! commits them with the model node as one batch under a compare-and-set on the model version.
//! An export (MEDULLA-CQ-ORCH-R702.10, R702.11) writes a document back from a stored model or
//! a stored workflow.

#![cfg(feature = "redb")]

mod common;
#[path = "common/test_support.rs"]
mod test_support;

use medulla_core::process_exchange::bpmn_graph::bpmn_mutations;
use medulla_core::process_exchange::bpmn_read::{parse_bpmn, BpmnLimits};
use medulla_core::process_exchange::dmn_graph::dmn_mutations;
use medulla_core::process_exchange::dmn_read::parse_dmn;
use medulla_core::process_exchange::xml_tree::XmlLimits;
use medulla_protocol_types::protocol::{Method, Response, ResultPayload};
use medulla_types::archimate_exchange::sha256_hex;
use medulla_types::process_exchange::{
    ExportSource, ProcessDocument, ProcessDocumentExported, ProcessDocumentFormat,
    ProcessDocumentImported,
};
use medulla_types::process_import::CanonicalWorkflow;

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

const LIMITS_DMN: &[u8] =
    include_bytes!("../crates/medulla-core/tests/fixtures/process_exchange/limits.dmn");
const WORKFLOW_JSON: &str = include_str!(
    "../crates/medulla-core/tests/fixtures/process_equivalence/invoice_approval.canonical.json"
);

fn import_document(bytes: &[u8], format: ProcessDocumentFormat, iri: &str) -> Method {
    Method::ProcessDocumentImport {
        document: ProcessDocument {
            schema_version: 1,
            format,
            document: bytes.to_vec(),
            source_ref: None,
        },
        model_iri: iri.into(),
        expected_version: None,
    }
}

fn export_method(source: ExportSource, format: ProcessDocumentFormat) -> Method {
    Method::ProcessDocumentExport { source, format }
}

fn model(iri: &str) -> ExportSource {
    ExportSource::Model {
        model_iri: iri.into(),
    }
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

// spec: MEDULLA-CQ-ORCH-R702.10
#[tokio::test(flavor = "multi_thread")]
async fn export_reads_model_and_workflow() {
    let (_dir, _backend, state) = test_support::durable_graph_state(
        SECRET,
        ENCRYPTION_KEY,
        "process-exchange-export",
        common::current_isolation(),
        GRAPH,
    )
    .await;
    let _: ProcessDocumentImported = payload(send(&state, 2, import_method(EVERY_KIND, None)).await);

    // A model exports as the document it was imported from, up to element order.
    let exported: ProcessDocumentExported = payload(
        send(
            &state,
            3,
            export_method(model(MODEL_IRI), ProcessDocumentFormat::Bpmn20),
        )
        .await,
    );
    assert_eq!(exported.document.format, ProcessDocumentFormat::Bpmn20);
    assert_eq!(exported.document_digest, sha256_hex(&exported.document.document));
    let original = parse_bpmn(EVERY_KIND, BpmnLimits::default()).unwrap();
    let written = parse_bpmn(&exported.document.document, BpmnLimits::default()).unwrap();
    assert_eq!(
        bpmn_mutations(&written, MODEL_IRI),
        bpmn_mutations(&original, MODEL_IRI),
        "the exported model maps onto the same graph writes"
    );

    // A re-import with fewer elements replaces the model: the export carries no stale element.
    let slim = br#"<definitions xmlns="http://www.omg.org/spec/BPMN/20100524/MODEL" id="defs"><process id="p"><startEvent id="s"/><endEvent id="e"/><sequenceFlow id="f" sourceRef="s" targetRef="e"/></process></definitions>"#;
    let _: ProcessDocumentImported = payload(
        send(&state, 4, import_document(slim, ProcessDocumentFormat::Bpmn20, MODEL_IRI)).await,
    );
    let replaced: ProcessDocumentExported = payload(
        send(
            &state,
            5,
            export_method(model(MODEL_IRI), ProcessDocumentFormat::Bpmn20),
        )
        .await,
    );
    let replaced = parse_bpmn(&replaced.document.document, BpmnLimits::default()).unwrap();
    assert_eq!(
        bpmn_mutations(&replaced, MODEL_IRI),
        bpmn_mutations(&parse_bpmn(slim, BpmnLimits::default()).unwrap(), MODEL_IRI)
    );

    // A stored workflow exports as a deployable document.
    let workflow: CanonicalWorkflow = serde_json::from_str(WORKFLOW_JSON).unwrap();
    let stored = send(
        &state,
        6,
        Method::ProcessModelImport {
            workflow,
            workflow_id: "invoice-approval".into(),
            expected_version: None,
        },
    )
    .await;
    assert!(stored.error.is_none(), "refused: {:?}", stored.error);
    let from_workflow: ProcessDocumentExported = payload(
        send(
            &state,
            7,
            export_method(
                ExportSource::Workflow {
                    workflow_id: "invoice-approval".into(),
                    version: 1,
                },
                ProcessDocumentFormat::Bpmn20,
            ),
        )
        .await,
    );
    let deployable =
        parse_bpmn(&from_workflow.document.document, BpmnLimits::default()).unwrap();
    let kinds: Vec<&str> = deployable.processes[0]
        .nodes
        .iter()
        .map(|node| node.kind.as_str())
        .collect();
    assert!(kinds.contains(&"startEvent") && kinds.contains(&"endEvent"));

    // An unknown model and an unknown workflow refuse without a write.
    let version = core_version(&state);
    let unknown = send(
        &state,
        8,
        export_method(model("urn:model:absent"), ProcessDocumentFormat::Bpmn20),
    )
    .await;
    assert!(unknown
        .error
        .as_deref()
        .is_some_and(|error| error.starts_with("PROCESS_IMPORT_UNKNOWN_MODEL")));
    assert_eq!(core_version(&state), version);
}

// spec: MEDULLA-CQ-ORCH-R702.11
#[tokio::test(flavor = "multi_thread")]
async fn dmn_export_reads_definitions() {
    let (_dir, _backend, state) = test_support::durable_graph_state(
        SECRET,
        ENCRYPTION_KEY,
        "process-exchange-dmn-export",
        common::current_isolation(),
        GRAPH,
    )
    .await;
    let iri = "urn:model:limits";
    let imported: ProcessDocumentImported = payload(
        send(&state, 2, import_document(LIMITS_DMN, ProcessDocumentFormat::Dmn13, iri)).await,
    );
    assert_eq!(imported.format, ProcessDocumentFormat::Dmn13);

    let exported: ProcessDocumentExported = payload(
        send(&state, 3, export_method(model(iri), ProcessDocumentFormat::Dmn13)).await,
    );
    assert_eq!(exported.document.format, ProcessDocumentFormat::Dmn13);
    assert_eq!(exported.document_digest, sha256_hex(&exported.document.document));
    let original = parse_dmn(LIMITS_DMN, XmlLimits::default()).unwrap();
    let written = parse_dmn(&exported.document.document, XmlLimits::default()).unwrap();
    assert_eq!(
        dmn_mutations(&written, iri),
        dmn_mutations(&original, iri),
        "the exported decision model maps onto the same graph writes"
    );

    // A BPMN model is not a DMN source, and a workflow is not either.
    let _: ProcessDocumentImported = payload(
        send(&state, 4, import_method(EVERY_KIND, None)).await,
    );
    for (id, source) in [
        (5, model(MODEL_IRI)),
        (
            6,
            ExportSource::Workflow {
                workflow_id: "invoice-approval".into(),
                version: 1,
            },
        ),
    ] {
        let refused = send(&state, id, export_method(source, ProcessDocumentFormat::Dmn13)).await;
        assert!(refused
            .error
            .as_deref()
            .is_some_and(|error| error.starts_with("PROCESS_EXPORT_NOT_DEPLOYABLE")));
    }
}

// spec: MEDULLA-CQ-ORCH-R702.12
#[tokio::test(flavor = "multi_thread")]
async fn document_refusals_leave_state_unchanged() {
    let (_dir, _backend, state) = test_support::durable_graph_state(
        SECRET,
        ENCRYPTION_KEY,
        "process-exchange-refusals",
        common::current_isolation(),
        GRAPH,
    )
    .await;
    let before = core_version(&state);
    let oversized = vec![b' '; 16 * 1024 * 1024 + 1];
    let cases: [(&[u8], &str); 4] = [
        (b"<definitions xmlns=\"http://www.omg.org/spec/BPMN/20100524/MODEL\">\n<process id=\"p\"></task></definitions>", "PROCESS_DOC_MALFORMED"),
        (br#"<!DOCTYPE d [<!ENTITY x "y">]><definitions xmlns="http://www.omg.org/spec/BPMN/20100524/MODEL" id="d"/>"#, "PROCESS_DOC_ENTITY_REFUSED"),
        (&oversized, "PROCESS_DOC_TOO_LARGE"),
        (br#"<definitions xmlns="urn:not-bpmn" id="d"/>"#, "PROCESS_DOC_NAMESPACE_UNKNOWN"),
    ];
    for (n, (bytes, code)) in cases.iter().enumerate() {
        let refused = send(&state, 2 + n as u64, import_method(bytes, None)).await;
        let error = refused.error.as_deref().unwrap_or_default();
        assert!(error.starts_with(code), "{code}: got {error}");
    }
    let malformed = send(&state, 10, import_method(cases[0].0, None)).await;
    assert!(
        malformed.error.as_deref().is_some_and(|e| e.contains("2:")),
        "the malformed refusal names the line and column"
    );

    // A workflow is not a DMN source.
    let not_deployable = send(
        &state,
        11,
        export_method(
            ExportSource::Workflow {
                workflow_id: "w".into(),
                version: 1,
            },
            ProcessDocumentFormat::Dmn13,
        ),
    )
    .await;
    assert!(not_deployable
        .error
        .as_deref()
        .is_some_and(|e| e.starts_with("PROCESS_EXPORT_NOT_DEPLOYABLE")));
    assert_eq!(core_version(&state), before, "a refusal writes nothing");
}

// spec: MEDULLA-CQ-ORCH-R702.13
#[tokio::test(flavor = "multi_thread")]
async fn model_keeps_document_and_canonical_digests() {
    let (_dir, _backend, state) = test_support::durable_graph_state(
        SECRET,
        ENCRYPTION_KEY,
        "process-exchange-digests",
        common::current_isolation(),
        GRAPH,
    )
    .await;
    let imported: ProcessDocumentImported =
        payload(send(&state, 2, import_method(EVERY_KIND, None)).await);
    assert_eq!(imported.source_document_digest, sha256_hex(EVERY_KIND));

    // The model node carries both digests, 64 lowercase hex characters each.
    let (document, canonical) = {
        let guard = state.try_read().unwrap();
        let core = &guard.registry.get(GRAPH).unwrap().core;
        let bytes = core.get_node_properties(&imported.model_node_id).unwrap();
        let properties = medulla_types::msgpack::decode_property_object(&bytes).unwrap();
        let text = |key: &str| properties[key].as_str().unwrap().to_string();
        (text("sourceDocumentDigest"), text("canonicalModelDigest"))
    };
    let is_hex = |text: &str| {
        text.len() == 64 && text.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    };
    assert_eq!(document, imported.source_document_digest);
    assert!(is_hex(&canonical));

    // The export answers the digest of the document it wrote and the stored canonical digest.
    let exported: ProcessDocumentExported = payload(
        send(
            &state,
            3,
            export_method(model(MODEL_IRI), ProcessDocumentFormat::Bpmn20),
        )
        .await,
    );
    assert_eq!(exported.canonical_model_digest, canonical);
    assert_eq!(exported.document_digest, sha256_hex(&exported.document.document));
}
