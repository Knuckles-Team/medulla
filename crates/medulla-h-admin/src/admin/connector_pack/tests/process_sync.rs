//! MEDULLA-CQ-ORCH-R703.5 and R703.6: the served `SourceSync` of process model definitions
//! through the real signed dispatch, with the process adapter.
//!
//! The host opens, fills and completes one approved foreign read per sync; `SourceSync`
//! takes the definitions exactly once and commits one batch of model nodes, element nodes,
//! flow edges and sync state. The engine is the shared sync harness of the connector-pack
//! tests.

use serde_json::{json, Value};

use medulla_protocol_types::protocol::Method;
use medulla_source_types::source_sync::SyncConflictReason;
use medulla_types::contract::Digest256;
use medulla_types::process_exchange::PROCESS_MODEL_CLASS_IRI;
use medulla_virtual_graph::process_sync::ProcessSyncAdapter;
use medulla_virtual_graph::source_sync::{SyncAdapter, SYNC_STATE_CLASS};

use super::sync_harness::{unique, SyncHarness, SyncSpec};

const CONNECTOR: &str = "r703-sync";
const GRAPH: &str = "process-sync";
const CONNECTION: &str = "urn:eg:fixture:process-sync:process-engine";
const SCOPE: &str = "urn:model:sync";
const KEY: &str = "bpmn20:invoice:1";
const AUTHORITY: &str =
    include_str!("../../../../../medulla-core/tests/fixtures/process_sync/authority.ttl");
const INVOICE: &str = include_str!(
    "../../../../../medulla-core/tests/fixtures/process_equivalence/invoice-approval.bpmn"
);

/// One definition as the process engine connector delivers it.
fn definition(revision: &str, document: &str) -> Value {
    json!({
        "system": "bpmn20",
        "model_key": "invoice",
        "version": "1",
        "revision": revision,
        "document": document,
    })
}

fn spec() -> SyncSpec {
    SyncSpec {
        connector: CONNECTOR,
        graph: GRAPH,
        connection: CONNECTION,
        entity: "definitions",
        key_field: "model_key",
        contract_class: "urn:ProcessModel",
        operation: "export_definitions",
        authority: AUTHORITY.to_string(),
        scope: Some(SCOPE),
        row_id: |record| {
            format!(
                "{}:{}:{}",
                record["system"].as_str().unwrap(),
                record["model_key"].as_str().unwrap(),
                record["version"].as_str().unwrap()
            )
        },
    }
}

// spec: MEDULLA-CQ-ORCH-R703.5
/// Verifies MEDULLA-CQ-ORCH-R703.5: every node an imported model wrote carries `syncedFrom`
/// with the connection IRI, and the sync state node of the definition keeps the connection,
/// the definition key and the source revision.
#[tokio::test]
async fn imported_model_cites_source_record() {
    let h = SyncHarness::new(spec()).await;
    let report = h
        .sync(PROCESS_MODEL_CLASS_IRI, &[definition("7", INVOICE)])
        .await;
    assert_eq!((report.applied, report.proposed), (1, 0), "{report:?}");

    let models = h.nodes_of("ProcessModel").await;
    assert_eq!(models.len(), 1, "one definition, one model node");
    let model = &models[0];
    assert_eq!(model["syncedFrom"], CONNECTION);
    assert_eq!(model["modelIri"], format!("{SCOPE}:bpmn20:invoice"));
    let element_ids: Vec<String> =
        serde_json::from_str(model["elementIds"].as_str().expect("the element ids")).unwrap();
    assert!(!element_ids.is_empty());
    let nodes = h.nodes().await;
    for id in &element_ids {
        assert_eq!(nodes[id]["syncedFrom"], CONNECTION, "element {id}");
    }

    let states = h.nodes_of(SYNC_STATE_CLASS).await;
    assert_eq!(states.len(), 1);
    assert_eq!(states[0]["syncConnection"], CONNECTION);
    assert_eq!(states[0]["syncRecordKey"], KEY);
    assert_eq!(states[0]["sourceRevision"], "7");
}

// spec: MEDULLA-CQ-ORCH-R703.6
/// Verifies MEDULLA-CQ-ORCH-R703.6: a definition edited in the graph and changed in the source
/// records one conflict, the graph model wins, and the outbound proposal does not carry the
/// source document.
#[tokio::test]
async fn both_sides_changed_records_conflict_and_graph_wins() {
    let h = SyncHarness::new(spec()).await;
    let first = h
        .sync(PROCESS_MODEL_CLASS_IRI, &[definition("7", INVOICE)])
        .await;
    assert_eq!(first.applied, 1, "{first:?}");

    // The graph side: rename the "Check invoice" task through the served batch.
    let model = h.nodes_of("ProcessModel").await.remove(0);
    let element_ids: Vec<String> =
        serde_json::from_str(model["elementIds"].as_str().unwrap()).unwrap();
    let nodes = h.nodes().await;
    let (task_id, property) = element_ids
        .iter()
        .find_map(|id| {
            nodes[id].iter().find_map(|(name, value)| {
                (value.as_str() == Some("Check invoice")).then(|| (id.clone(), name.clone()))
            })
        })
        .expect("the named task node");
    let edited = h
        .served
        .call_on(
            GRAPH,
            &unique("edit"),
            Method::BatchUpdate {
                operations_msgpack: rmp_serde::to_vec_named(&vec![json!({
                    "op": "upsert_node",
                    "id": task_id,
                    "properties": {property.clone(): "Check (graph)"},
                })])
                .unwrap(),
            },
        )
        .await;
    assert!(
        edited.refusal_text().is_none(),
        "{:?}",
        edited.refusal_text()
    );

    // The source side: the same definition at revision 8 with a different task name.
    let source_document = INVOICE.replace("name=\"Check invoice\"", "name=\"Check (source)\"");
    let source = definition("8", &source_document);
    let second = h.sync(PROCESS_MODEL_CLASS_IRI, &[source.clone()]).await;

    assert_eq!(second.conflicts.len(), 1, "{:?}", second.conflicts);
    assert_eq!(second.conflicts[0].record_key, KEY);
    assert_eq!(second.conflicts[0].reason, SyncConflictReason::BothChanged);
    assert_eq!(second.conflicts[0].rule, "system_of_record_wins");
    assert_eq!((second.applied, second.proposed), (0, 1), "{second:?}");
    assert_eq!(second.outbound.len(), 1);
    assert_eq!(second.outbound[0].record_key, KEY);

    // The graph model wins: the node keeps the graph edit, and the proposal is not the source.
    let nodes = h.nodes().await;
    assert_eq!(nodes[&task_id][&property], "Check (graph)");
    let adapter = ProcessSyncAdapter {
        model_iri: SCOPE.to_string(),
    };
    let source_payload = adapter.outbound_payload(KEY, &source);
    let source_digest = Digest256::sha256(source_payload.to_string().as_bytes()).to_hex();
    assert_ne!(second.outbound[0].payload_digest, source_digest);

    // The same state proposes the same document again.
    let third = h.sync(PROCESS_MODEL_CLASS_IRI, &[source]).await;
    assert_eq!(third.outbound.len(), 1);
    assert_eq!(
        third.outbound[0].payload_digest,
        second.outbound[0].payload_digest
    );
}
