//! The process model sync adapter (MEDULLA-CQ-ORCH-R703.2).

use medulla_virtual_graph::process_sync::ProcessSyncAdapter;
use medulla_virtual_graph::source_sync::{GraphEdge, GraphEdges, GraphNodes, SyncAdapter};
use serde_json::{json, Value};

const BPMN: &str = include_str!("../../medulla-core/tests/fixtures/process_equivalence/invoice_approval.bpmn");
const EPC: &str =
    include_str!("../../medulla-core/tests/fixtures/process_equivalence/invoice_approval.epc.canonical.json");

fn adapter() -> ProcessSyncAdapter {
    ProcessSyncAdapter {
        model_iri: "engine".into(),
    }
}

fn bpmn_record(version: &str) -> Value {
    json!({"system": "bpmn20", "model_key": "invoice", "version": version, "document": BPMN})
}

fn epc_record() -> Value {
    json!({
        "system": "aris_epc",
        "model_key": "invoice",
        "version": "1",
        "canonical": serde_json::from_str::<Value>(EPC).unwrap(),
    })
}

// spec: MEDULLA-CQ-ORCH-R703.2
#[test]
fn process_adapter_keys_and_digests_are_stable() {
    let adapter = adapter();
    assert_eq!(
        adapter.record_key(&bpmn_record("3")).as_deref(),
        Some("bpmn20:invoice:3")
    );
    // A record missing the system, the key or the version has no key.
    assert_eq!(adapter.record_key(&json!({"model_key": "a", "version": "1"})), None);
    assert_eq!(adapter.record_key(&json!({"system": "dmn", "version": "1"})), None);
    assert_eq!(adapter.record_key(&json!({"system": "dmn", "model_key": "a"})), None);

    // Equal documents give equal digests, a different version changes the key only.
    let digest = adapter.record_digest(&bpmn_record("3"));
    assert_eq!(digest.len(), 64, "{digest}");
    assert_eq!(digest, adapter.record_digest(&bpmn_record("4")));

    // One process as BPMN and as its canonical EPC form compares equal.
    assert_eq!(digest, adapter.record_digest(&epc_record()));
}

// spec: MEDULLA-CQ-ORCH-R703.2
#[test]
fn unparseable_document_digests_as_invalid() {
    let adapter = adapter();
    let broken = json!({"system": "bpmn20", "model_key": "x", "version": "1", "document": "<not-xml"});
    assert!(adapter.record_digest(&broken).starts_with("invalid:"));
    let unknown = json!({"system": "visio", "model_key": "x", "version": "1"});
    assert_eq!(adapter.record_digest(&unknown), "invalid:PROCESS_SYNC_UNKNOWN_SYSTEM");
    let mut bad_canonical = epc_record();
    bad_canonical["canonical"]["digest"] = json!("0".repeat(64));
    assert_eq!(
        adapter.record_digest(&bad_canonical),
        "invalid:PROCESS_IMPORT_DIGEST_MISMATCH"
    );
    // The broken record writes nothing inbound.
    assert!(adapter.inbound_mutations("bpmn20:x:1", &broken).is_empty());
    assert!(adapter.inbound_edges("bpmn20:x:1", &broken).is_empty());
}

// spec: MEDULLA-CQ-ORCH-R703.2
#[test]
fn graph_records_rebuild_the_record_from_nodes_and_edges() {
    let adapter = adapter();
    let record = bpmn_record("3");
    let key = adapter.record_key(&record).unwrap();
    // Apply the record inbound, then read it back from the nodes and the edges it wrote.
    let mut nodes = GraphNodes::new();
    for mutation in adapter.inbound_mutations(&key, &record) {
        nodes.insert(mutation.node_id, mutation.properties);
    }
    let edges: GraphEdges = adapter
        .inbound_edges(&key, &record)
        .into_iter()
        .map(|edge| {
            let mut properties = serde_json::Map::new();
            properties.insert("relationship".into(), json!(edge.relationship));
            GraphEdge {
                source: edge.source,
                target: edge.target,
                properties,
            }
        })
        .collect();
    assert!(!edges.is_empty(), "the structure of a process is held by edges");
    let rebuilt = adapter.graph_records(&nodes, &edges);
    let back = rebuilt.get(&key).expect("the model is rebuilt under its key");
    assert_eq!(adapter.record_digest(back), adapter.record_digest(&record));
    // Without its edges the model cannot be rebuilt to the same process.
    let without = adapter.graph_records(&nodes, &GraphEdges::new());
    assert!(
        without
            .get(&key)
            .is_none_or(|record| adapter.record_digest(record) != adapter.record_digest(&bpmn_record("3")))
    );
}
