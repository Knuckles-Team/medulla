//! Dataset lineage ingest (MEDULLA-UNIFIED-DATA-PLANE-R702.3 / R702.6).
//!
//! `handle_lineage_ingest` drains one served batch of lineage run events with
//! [`LineageDrain`] and lowers the result into ONE write-set that commits
//! through the same ChangeEnvelope authority as `EnrichDocument` -- no second
//! graph writer. Every dataset is a content-addressed `Dataset` node, every
//! edge a `derivesInto` edge from input to output, and every event that cannot
//! map one `QuarantinedLineageEvent` node keyed by its payload digest. The
//! envelope id is the write-set digest, so a replayed batch adds no edge, and
//! an edge the graph already holds is never lowered again.

use std::collections::BTreeMap;

use medulla_catalog_types::lineage::{
    DatasetId, LineageDrain, LineageEdge, QuarantinedLineageEvent, DATASET_LABEL,
    DERIVES_RELATIONSHIP, LINEAGE_EVENT_MALFORMED, QUARANTINED_LABEL,
};
pub use medulla_catalog_types::lineage::{LineageEvent, LineageIngestReceipt};
use medulla_protocol_types::protocol::Method;
use serde_json::{json, Value};

use super::repository_index::{EnvelopeLabels, IndexWriteSet};

/// The labels a lineage envelope is sealed under.
pub const LABELS: EnvelopeLabels = EnvelopeLabels {
    version_type: "lineage-drain-sha256",
    policy_version: "lineage-drain",
};
const ENVELOPE_PREFIX: &str = "lineage-drain:";

fn encode(value: &Value) -> Result<Vec<u8>, String> {
    rmp_serde::to_vec_named(value).map_err(|error| format!("lineage encoding failed: {error}"))
}

fn node(id: String, properties: Value) -> Result<Method, String> {
    Ok(Method::AddNode {
        node_id: id,
        properties_msgpack: encode(&properties)?,
    })
}

/// The quarantine node id of one event: keyed by its payload digest, so the
/// same malformed payload is kept once.
pub fn quarantine_id(quarantined: &QuarantinedLineageEvent) -> String {
    format!("lineage-quarantine:{}", quarantined.payload_digest)
}

/// Refuse a batch holding an event that cannot be keyed, before any write.
fn check_keys(events: &[LineageEvent]) -> Result<(), String> {
    match events
        .iter()
        .position(|event| event.event_id.trim().is_empty())
    {
        Some(index) => Err(format!(
            "{LINEAGE_EVENT_MALFORMED}: event {index} has a blank event_id"
        )),
        None => Ok(()),
    }
}

/// The dataset nodes of every event that maps, by id, with their reference.
fn datasets(events: &[LineageEvent]) -> BTreeMap<DatasetId, (String, String)> {
    let mut out = BTreeMap::new();
    for event in events {
        for dataset in event.inputs.iter().chain(&event.outputs) {
            if let Ok(id) = DatasetId::address(&dataset.namespace, &dataset.name) {
                out.entry(id)
                    .or_insert_with(|| (dataset.namespace.clone(), dataset.name.clone()));
            }
        }
    }
    out
}

/// Drain `events` and lower them into one write-set. `known_edge` answers
/// whether the graph already holds an input-to-output edge; such an edge is
/// not lowered again. Refuses `LINEAGE_EVENT_MALFORMED` before any write.
pub fn handle_lineage_ingest(
    events: Vec<LineageEvent>,
    known_edge: &dyn Fn(&str, &str) -> bool,
) -> Result<(IndexWriteSet, LineageIngestReceipt), String> {
    check_keys(&events)?;
    let received = events.len();
    let nodes = datasets(&events);
    let mut drain = LineageDrain::new(received);
    let mut pending = events;
    let batch = drain.drain(&mut pending);
    let edges: Vec<&LineageEdge> = batch
        .new_edges
        .iter()
        .filter(|edge| !known_edge(edge.dataset_in.as_str(), edge.dataset_out.as_str()))
        .collect();
    let mut methods = Vec::new();
    for (id, (namespace, name)) in &nodes {
        let used = edges
            .iter()
            .any(|edge| &edge.dataset_in == id || &edge.dataset_out == id);
        if used {
            methods.push(node(
                id.as_str().to_string(),
                json!({ "type": DATASET_LABEL, "namespace": namespace, "name": name }),
            )?);
        }
    }
    for edge in &edges {
        methods.push(Method::AddEdge {
            source_id: edge.dataset_in.as_str().to_string(),
            target_id: edge.dataset_out.as_str().to_string(),
            properties_msgpack: encode(
                &json!({ "relationship": DERIVES_RELATIONSHIP, "run": edge.run }),
            )?,
        });
    }
    for quarantined in &batch.quarantined {
        methods.push(node(
            quarantine_id(quarantined),
            json!({
                "type": QUARANTINED_LABEL,
                "event_id": quarantined.event_id,
                "payload_digest": quarantined.payload_digest,
                "code": quarantined.reason.code(),
                "reason": serde_json::to_value(&quarantined.reason).unwrap_or(Value::Null),
            }),
        )?);
    }
    let write_set = IndexWriteSet::from_methods(ENVELOPE_PREFIX, b"eg/lineage-drain", methods)?;
    let receipt = LineageIngestReceipt {
        received: received as u32,
        edges: edges.len() as u32,
        quarantined: batch.quarantined.len() as u32,
        replayed: batch.replayed as u32,
        envelope_id: write_set.envelope_id.clone(),
    };
    Ok((write_set, receipt))
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_catalog_types::lineage::{DatasetRef, EtlRun};

    fn event(id: &str, input: &str, output: &str) -> LineageEvent {
        LineageEvent {
            event_id: id.into(),
            run: EtlRun {
                run_id: format!("run-{id}"),
                job: "job".into(),
                started_at_ms: 1,
                ended_at_ms: Some(2),
            },
            inputs: vec![DatasetRef {
                namespace: "ns".into(),
                name: input.into(),
            }],
            outputs: vec![DatasetRef {
                namespace: "ns".into(),
                name: output.into(),
            }],
        }
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R702.6
    #[test]
    fn replay_lowers_no_edge_and_malformed_events_quarantine() {
        let events = vec![event("a", "x", "y"), event("b", "y", "z")];
        let (first, receipt) = handle_lineage_ingest(events.clone(), &|_, _| false).unwrap();
        assert_eq!(receipt.edges, 2);
        let (again, _) = handle_lineage_ingest(events, &|_, _| false).unwrap();
        assert_eq!(first.envelope_id, again.envelope_id);
        let (held, receipt) =
            handle_lineage_ingest(vec![event("a", "x", "y")], &|_, _| true).unwrap();
        assert_eq!(receipt.edges, 0);
        assert!(held.methods.is_empty());
        let (_, receipt) =
            handle_lineage_ingest(vec![event("c", "bad name", "y")], &|_, _| false).unwrap();
        assert_eq!(receipt.quarantined, 1);
        let blank = handle_lineage_ingest(vec![event(" ", "x", "y")], &|_, _| false);
        assert!(blank.unwrap_err().starts_with(LINEAGE_EVENT_MALFORMED));
    }
}
