//! Admission of one harness session batch (MEDULLA-ORCH-VOCAB-R009.101.1).
//!
//! The index is a projection over committed `RunTrace` nodes: this module verifies every
//! record against the run trace it names and plans the index writes as `BatchUpdate`
//! operations; the router's gateway route commits them. Nothing is written on the first
//! refusal, and a replay of an identical batch plans no write at all. All state is in the
//! graph, so a replay after a restart behaves the same.

use medulla_graph_core::graph::GraphCore;
use medulla_types::harness_session::{
    counter_node_id, is_hex64, receipt_node_id, session_node_id, HarnessSessionRecord,
    IndexHarnessSessionBatch, IndexHarnessSessionBatchResult, COUNTER_LABEL, EDGE_INDEXES_RUN,
    HARNESS_SESSION_MANIFEST_REVISION, RECEIPT_LABEL, SESSION_CONTENT_CHANGED, SESSION_LABEL,
    SESSION_MANIFEST_UNSUPPORTED, SESSION_TRACE_UNKNOWN,
};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

/// The properties a run trace digest leaves out: its class, tenant stamp and revision.
const DIGEST_EXCLUDED: [&str; 3] = ["node_type", "tenant", "run_revision"];

fn refuse(code: &str, detail: impl std::fmt::Display) -> String {
    format!("{code}: {detail}")
}

fn canonical_json(value: &Value, out: &mut String) {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push('{');
            for (index, key) in keys.into_iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                out.push_str(&Value::String(key.clone()).to_string());
                out.push(':');
                canonical_json(&map[key], out);
            }
            out.push('}');
        }
        Value::Array(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                canonical_json(item, out);
            }
            out.push(']');
        }
        other => out.push_str(&other.to_string()),
    }
}

/// The digest of a stored run trace: the lowercase hex SHA-256 of the canonical JSON (object
/// keys sorted at every depth, no whitespace) of the node's property map with `node_type`,
/// `tenant` and `run_revision` removed. A connector computes the same function over the same
/// node (`contracts/scion-harness-session-ingest.md`).
pub fn run_trace_digest(properties: &Map<String, Value>) -> String {
    let kept: Map<String, Value> = properties
        .iter()
        .filter(|(key, _)| !DIGEST_EXCLUDED.contains(&key.as_str()))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    let mut text = String::new();
    canonical_json(&Value::Object(kept), &mut text);
    hex::encode(Sha256::digest(text.as_bytes()))
}

/// One planned batch: the operations to commit (empty for an identical replay), the answer and
/// the index revision the plan was made against.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionAdmission {
    /// The operations to commit; empty for an identical replay.
    pub operations: Vec<Value>,
    /// The answer of the batch.
    pub result: IndexHarnessSessionBatchResult,
    /// The counter revision the plan read; the commit refuses when it moved.
    pub revision_before: u64,
}

fn properties_of(core: &GraphCore, id: &str) -> Option<Map<String, Value>> {
    let blob = core.get_node_properties(id)?;
    medulla_types::msgpack::decode_property_object(&blob).ok()
}

fn text<'a>(properties: &'a Map<String, Value>, name: &str) -> &'a str {
    properties.get(name).and_then(Value::as_str).unwrap_or_default()
}

fn number(properties: &Map<String, Value>, name: &str) -> Option<u64> {
    match properties.get(name)? {
        Value::String(text) => text.parse().ok(),
        Value::Number(number) => number.as_u64(),
        _ => None,
    }
}

/// The counter revision of `tenant`: 0 when none is stored.
pub fn index_revision(core: &GraphCore, tenant: &str) -> u64 {
    properties_of(core, &counter_node_id(tenant))
        .and_then(|properties| number(&properties, "revision"))
        .unwrap_or(0)
}

/// The run trace `id` the tenant holds: its properties, or `None` when it is absent, of
/// another class or of another tenant (all read the same).
fn run_trace(core: &GraphCore, tenant: &str, id: &str) -> Option<Map<String, Value>> {
    let properties = properties_of(core, id)?;
    let classes = [text(&properties, "node_type"), text(&properties, "type")];
    if !classes.contains(&"RunTrace") {
        return None;
    }
    let owners = [text(&properties, "tenant"), text(&properties, "tenant_id")];
    if owners.iter().any(|owner| !owner.is_empty() && *owner != tenant) {
        return None;
    }
    Some(properties)
}

fn record_fields_equal(node: &Map<String, Value>, record: &HarnessSessionRecord) -> bool {
    text(node, "harness_id") == record.harness_id
        && number(node, "adapter_revision") == Some(u64::from(record.adapter_revision))
        && text(node, "source_session_id") == record.source_session_id
        && text(node, "run_trace_id") == record.run_trace_id
        && number(node, "run_trace_revision") == Some(record.run_trace_revision)
        && text(node, "trace_digest") == record.trace_digest
        && number(node, "source_sequence") == Some(record.source_sequence)
}

fn session_operation(
    tenant: &str,
    batch: &IndexHarnessSessionBatch,
    record: &HarnessSessionRecord,
) -> Value {
    json!({
        "op": "upsert_node",
        "id": session_node_id(tenant, &record.harness_id, &record.source_session_id),
        "properties": {
            "type": SESSION_LABEL,
            "tenant_id": tenant,
            "status": "indexed",
            "harness_id": record.harness_id,
            "adapter_revision": record.adapter_revision.to_string(),
            "source_session_id": record.source_session_id,
            "run_trace_id": record.run_trace_id,
            "run_trace_revision": record.run_trace_revision.to_string(),
            "trace_digest": record.trace_digest,
            "source_sequence": record.source_sequence.to_string(),
            "manifest_id": batch.manifest_id,
            "sync_receipt_id": batch.sync_receipt_id,
        },
    })
}

/// Admit `batch` for `tenant` against the graph, writing nothing. `batch_max` is the knob
/// `orchestration_limits.session_batch_max`. Every record is verified, in this order, before
/// any write is planned: the manifest and digests (`SESSION_MANIFEST_UNSUPPORTED`), the run
/// trace the tenant holds (`SESSION_TRACE_UNKNOWN`) and its revision and digest
/// (`SESSION_CONTENT_CHANGED`).
pub fn admit_batch(
    core: &GraphCore,
    tenant: &str,
    batch: &IndexHarnessSessionBatch,
    batch_max: usize,
) -> Result<SessionAdmission, String> {
    if batch.tenant_id != tenant {
        return Err(refuse(
            SESSION_TRACE_UNKNOWN,
            "the batch names no run trace of this tenant",
        ));
    }
    batch.check_bounds(batch_max)?;
    let unsupported = |detail: &str| refuse(SESSION_MANIFEST_UNSUPPORTED, detail);
    if batch.manifest_revision != HARNESS_SESSION_MANIFEST_REVISION {
        return Err(unsupported(&format!(
            "manifest revision {} is not read; the engine reads {HARNESS_SESSION_MANIFEST_REVISION}",
            batch.manifest_revision
        )));
    }
    if !is_hex64(&batch.manifest_digest) || !is_hex64(&batch.sync_receipt_digest) {
        return Err(unsupported("a manifest or receipt digest is not 64 lowercase hex"));
    }
    for record in &batch.records {
        if record.adapter_revision == 0 || !is_hex64(&record.trace_digest) {
            return Err(unsupported(&format!(
                "session {} carries adapter revision 0 or a malformed trace digest",
                record.source_session_id
            )));
        }
    }
    // Verify every record against its run trace before anything is planned.
    for record in &batch.records {
        let trace = run_trace(core, tenant, &record.run_trace_id).ok_or_else(|| {
            refuse(
                SESSION_TRACE_UNKNOWN,
                format!("{} is no run trace of this tenant", record.run_trace_id),
            )
        })?;
        let revision = number(&trace, "run_revision").unwrap_or(1);
        if revision != record.run_trace_revision || run_trace_digest(&trace) != record.trace_digest
        {
            return Err(refuse(
                SESSION_CONTENT_CHANGED,
                format!(
                    "run trace {} changed since the connector read it",
                    record.run_trace_id
                ),
            ));
        }
    }
    let receipt_id = receipt_node_id(tenant, &batch.sync_receipt_id);
    let mut receipt_known = false;
    if let Some(existing) = properties_of(core, &receipt_id) {
        if text(&existing, "sync_receipt_digest") != batch.sync_receipt_digest {
            return Err(refuse(
                SESSION_CONTENT_CHANGED,
                format!("receipt {} is recorded with another digest", batch.sync_receipt_id),
            ));
        }
        receipt_known = true;
    }
    let mut operations: Vec<Value> = Vec::new();
    let mut edges: Vec<Value> = Vec::new();
    let (mut indexed, mut unchanged, mut unavailable) = (0u32, 0u32, 0u32);
    for record in &batch.records {
        let id = session_node_id(tenant, &record.harness_id, &record.source_session_id);
        match properties_of(core, &id) {
            Some(existing) if text(&existing, "status") == "indexed" => {
                let held = number(&existing, "source_sequence").unwrap_or(0);
                if held == record.source_sequence && record_fields_equal(&existing, record) {
                    unchanged += 1;
                    continue;
                }
                if record.source_sequence <= held {
                    return Err(refuse(
                        SESSION_CONTENT_CHANGED,
                        format!(
                            "session {} arrives at sequence {} over the held {held} with other content",
                            record.source_session_id, record.source_sequence
                        ),
                    ));
                }
            }
            _ => {}
        }
        operations.push(session_operation(tenant, batch, record));
        edges.push(json!({
            "op": "upsert_edge",
            "source": id,
            "target": record.run_trace_id,
            "properties": {"relationship": EDGE_INDEXES_RUN},
        }));
        indexed += 1;
    }
    for entry in &batch.unavailable {
        let id = session_node_id(tenant, &entry.harness_id, &entry.source_session_id);
        match properties_of(core, &id) {
            // A session already indexed is never downgraded to unavailable.
            Some(existing) if text(&existing, "status") == "indexed" => {
                unchanged += 1;
                continue;
            }
            Some(existing)
                if text(&existing, "status") == "unavailable"
                    && text(&existing, "reason") == entry.reason =>
            {
                unchanged += 1;
                continue;
            }
            _ => {}
        }
        operations.push(json!({
            "op": "upsert_node",
            "id": id,
            "properties": {
                "type": SESSION_LABEL,
                "tenant_id": tenant,
                "status": "unavailable",
                "harness_id": entry.harness_id,
                "source_session_id": entry.source_session_id,
                "reason": entry.reason,
                "source_sequence": "0",
                "manifest_id": batch.manifest_id,
                "sync_receipt_id": batch.sync_receipt_id,
            },
        }));
        unavailable += 1;
    }
    let revision_before = index_revision(core, tenant);
    let changed = !operations.is_empty() || !receipt_known;
    if !changed {
        return Ok(SessionAdmission {
            operations: Vec::new(),
            result: IndexHarnessSessionBatchResult {
                indexed,
                unchanged,
                unavailable,
                index_revision: revision_before,
            },
            revision_before,
        });
    }
    let revision = revision_before + 1;
    operations.extend(edges);
    if !receipt_known {
        operations.push(json!({
            "op": "upsert_node",
            "id": receipt_id,
            "properties": {
                "type": RECEIPT_LABEL,
                "tenant_id": tenant,
                "manifest_id": batch.manifest_id,
                "manifest_revision": batch.manifest_revision.to_string(),
                "manifest_digest": batch.manifest_digest,
                "declared_total": batch.declared_total.to_string(),
                "sync_receipt_digest": batch.sync_receipt_digest,
                "source_cursor": batch.source_cursor,
                "index_revision": revision.to_string(),
            },
        }));
    }
    operations.push(json!({
        "op": "upsert_node",
        "id": counter_node_id(tenant),
        "properties": {
            "type": COUNTER_LABEL,
            "tenant_id": tenant,
            "revision": revision.to_string(),
        },
    }));
    Ok(SessionAdmission {
        operations,
        result: IndexHarnessSessionBatchResult {
            indexed,
            unchanged,
            unavailable,
            index_revision: revision,
        },
        revision_before,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_types::harness_session::UnavailableSession;

    fn put(core: &GraphCore, id: &str, properties: Value) {
        core.add_node(id.to_string(), rmp_serde::to_vec_named(&properties).unwrap());
    }

    fn trace_properties(tenant: &str) -> Value {
        json!({"node_type": "RunTrace", "tenant": tenant, "status": "completed", "cost": 3})
    }

    fn batch_for(core: &GraphCore, tenant: &str) -> IndexHarnessSessionBatch {
        let trace = properties_of(core, "runtrace:s-1").unwrap();
        IndexHarnessSessionBatch {
            tenant_id: tenant.into(),
            manifest_id: "m1".into(),
            manifest_revision: 1,
            manifest_digest: "ab".repeat(32),
            declared_total: 2,
            sync_receipt_id: "r1".into(),
            sync_receipt_digest: "cd".repeat(32),
            source_cursor: "c1".into(),
            records: vec![HarnessSessionRecord {
                harness_id: "h".into(),
                adapter_revision: 1,
                source_session_id: "s-1".into(),
                run_trace_id: "runtrace:s-1".into(),
                run_trace_revision: 1,
                trace_digest: run_trace_digest(&trace),
                source_sequence: 5,
            }],
            unavailable: vec![UnavailableSession {
                harness_id: "h".into(),
                source_session_id: "s-2".into(),
                reason: "unreadable".into(),
            }],
        }
    }

    /// Apply the planned operations the way the gateway's `batch_update` does.
    fn apply(core: &GraphCore, admission: &SessionAdmission) {
        for operation in &admission.operations {
            if operation["op"] == "upsert_node" {
                put(core, operation["id"].as_str().unwrap(), operation["properties"].clone());
            }
        }
    }

    // spec: MEDULLA-ORCH-VOCAB-R009.101.1
    #[test]
    fn digest_ignores_class_tenant_and_revision_and_key_order() {
        let a = json!({"b": 1, "a": {"y": 2, "x": 1}, "node_type": "RunTrace", "tenant": "t1"});
        let b = json!({"a": {"x": 1, "y": 2}, "b": 1, "tenant": "t2", "run_revision": 9});
        assert_eq!(
            run_trace_digest(a.as_object().unwrap()),
            run_trace_digest(b.as_object().unwrap())
        );
        let c = json!({"a": {"x": 1, "y": 3}, "b": 1});
        assert_ne!(
            run_trace_digest(a.as_object().unwrap()),
            run_trace_digest(c.as_object().unwrap())
        );
    }

    // spec: MEDULLA-ORCH-VOCAB-R009.101.1
    #[test]
    fn replay_is_idempotent_and_changed_content_refuses() {
        let core = GraphCore::new();
        put(&core, "runtrace:s-1", trace_properties("t1"));
        let batch = batch_for(&core, "t1");
        let first = admit_batch(&core, "t1", &batch, 500).unwrap();
        assert_eq!(
            (first.result.indexed, first.result.unavailable, first.result.index_revision),
            (1, 1, 1)
        );
        apply(&core, &first);
        // An identical replay plans nothing and keeps the revision.
        let replay = admit_batch(&core, "t1", &batch, 500).unwrap();
        assert!(replay.operations.is_empty());
        assert_eq!((replay.result.unchanged, replay.result.index_revision), (2, 1));
        // Equal sequence, other digest of the run trace: the trace changed.
        put(
            &core,
            "runtrace:s-1",
            json!({"node_type": "RunTrace", "tenant": "t1", "status": "failed", "cost": 3}),
        );
        let changed = admit_batch(&core, "t1", &batch, 500).unwrap_err();
        assert!(changed.starts_with("SESSION_CONTENT_CHANGED"), "{changed}");
        // The receipt id under another digest is refused.
        let mut other = batch.clone();
        other.sync_receipt_digest = "ee".repeat(32);
        put(&core, "runtrace:s-1", trace_properties("t1"));
        assert!(admit_batch(&core, "t1", &other, 500)
            .unwrap_err()
            .starts_with("SESSION_CONTENT_CHANGED"));
        // A lower source sequence than the held one is refused; a higher one replaces.
        let mut lower = batch.clone();
        lower.records[0].source_sequence = 4;
        assert!(admit_batch(&core, "t1", &lower, 500)
            .unwrap_err()
            .starts_with("SESSION_CONTENT_CHANGED"));
        let mut higher = batch.clone();
        higher.records[0].source_sequence = 6;
        higher.sync_receipt_id = "r2".into();
        let replaced = admit_batch(&core, "t1", &higher, 500).unwrap();
        assert_eq!((replaced.result.indexed, replaced.result.index_revision), (1, 2));
    }

    // spec: MEDULLA-ORCH-VOCAB-R009.101.1
    #[test]
    fn unknown_trace_unsupported_manifest_and_foreign_tenant_refuse() {
        let core = GraphCore::new();
        put(&core, "runtrace:s-1", trace_properties("t1"));
        let batch = batch_for(&core, "t1");
        let mut unknown = batch.clone();
        unknown.records[0].run_trace_id = "runtrace:absent".into();
        assert!(admit_batch(&core, "t1", &unknown, 500)
            .unwrap_err()
            .starts_with("SESSION_TRACE_UNKNOWN"));
        let mut revision = batch.clone();
        revision.manifest_revision = 2;
        assert!(admit_batch(&core, "t1", &revision, 500)
            .unwrap_err()
            .starts_with("SESSION_MANIFEST_UNSUPPORTED"));
        let mut digest = batch.clone();
        digest.records[0].trace_digest = "AB".repeat(32);
        assert!(admit_batch(&core, "t1", &digest, 500)
            .unwrap_err()
            .starts_with("SESSION_MANIFEST_UNSUPPORTED"));
        // The same run trace id read as another tenant is absent, and a batch naming another
        // tenant than the verified one reads as absent too.
        assert!(admit_batch(&core, "t2", &batch_for(&core, "t2"), 500)
            .unwrap_err()
            .starts_with("SESSION_TRACE_UNKNOWN"));
        assert!(admit_batch(&core, "t2", &batch, 500)
            .unwrap_err()
            .starts_with("SESSION_TRACE_UNKNOWN"));
        // Nothing was planned by any refusal.
        assert_eq!(index_revision(&core, "t1"), 0);
    }
}
