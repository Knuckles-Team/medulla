//! The ArchiMate element sync adapter (MEDULLA-CQ-ORCH-R705.7).

use medulla_virtual_graph::architecture_sync::ArchitectureSyncAdapter;
use medulla_virtual_graph::source_sync::SyncAdapter;
use serde_json::json;

fn adapter() -> ArchitectureSyncAdapter {
    ArchitectureSyncAdapter {
        model_id: "id-model".into(),
    }
}

fn element() -> serde_json::Value {
    json!({
        "identifier": "id-proc",
        "element_type": "BusinessProcess",
        "name": "Order to cash",
        "properties": {"pd-tier": "gold"},
        "relationships": [
            {"identifier": "id-r2", "relationship_type": "Access", "source": "id-proc", "target": "id-obj"},
            {"identifier": "id-r1", "relationship_type": "Serving", "source": "id-app", "target": "id-proc"}
        ]
    })
}

// spec: MEDULLA-CQ-ORCH-R705.7
#[test]
fn adapter_keys_elements_by_identifier() {
    let adapter = adapter();
    let record = element();
    assert_eq!(adapter.record_key(&record).as_deref(), Some("id-proc"));
    assert_eq!(adapter.record_key(&json!({"name": "no id"})), None);
    assert_eq!(adapter.record_key(&json!({"identifier": ""})), None);

    // The digest is stable and ignores relationship order.
    let digest = adapter.record_digest(&record);
    assert_eq!(digest.len(), 64);
    let mut reordered = element();
    reordered["relationships"].as_array_mut().unwrap().reverse();
    assert_eq!(adapter.record_digest(&reordered), digest);

    // It covers type, name, properties and relationships.
    for (field, value) in [
        ("element_type", json!("BusinessFunction")),
        ("name", json!("Renamed")),
        ("properties", json!({"pd-tier": "silver"})),
        ("relationships", json!([])),
    ] {
        let mut changed = element();
        changed[field] = value;
        assert_ne!(adapter.record_digest(&changed), digest, "{field}");
    }

    // An inbound change writes the node the exchange mapping writes.
    let writes = adapter.inbound_mutations("id-proc", &record);
    assert_eq!(writes.len(), 1);
    assert_eq!(writes[0].node_id, "archimate:id-model:id-proc");
    assert_eq!(writes[0].properties["archimateIdentifier"], "id-proc");
    assert_eq!(writes[0].properties["name"], "Order to cash");
    assert_eq!(writes[0].properties["prop:pd-tier"], "gold");

    // An outbound payload carries the same fields, relationships in order.
    let payload = adapter.outbound_payload("id-proc", &record);
    assert_eq!(payload["identifier"], "id-proc");
    assert_eq!(payload["relationships"][0]["identifier"], "id-r1");
    assert_eq!(adapter.record_digest(&payload), digest);
}
