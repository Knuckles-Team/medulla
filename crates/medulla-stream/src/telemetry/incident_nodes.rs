//! The stored shape of a served incident (MEDULLA-UNIFIED-DATA-PLANE-R760.3): how an
//! [`IncidentRecord`] and its `HealthAnomaly` evidence become graph nodes and edges, and
//! how a stored `Incident` node reads back. The served incident methods and the quality
//! evaluation job (MEDULLA-UNIFIED-DATA-PLANE-R762.3) both write incidents through these
//! functions, so there is one stored shape.

use medulla_catalog_types::incident::{AnomalyReading, IncidentRecord};
use medulla_types::contract::Digest256;
use serde_json::{json, Map, Value};

/// The node type of a served incident.
pub const INCIDENT_TYPE: &str = "Incident";
/// The node type of one evidence reading.
pub const HEALTH_ANOMALY_TYPE: &str = "HealthAnomaly";
/// The property holding the canonical JSON of the [`IncidentRecord`]; an
/// `Incident` node without it (a CEP-derived fact) is not a served incident.
pub const INCIDENT_RECORD_PROPERTY: &str = "incident_record";
/// Incident → evidence edge relation.
pub const HAS_EVIDENCE: &str = "hasEvidence";
/// Most node and edge operations one incident write carries.
pub const MAX_INCIDENT_OPERATIONS: usize = 50_000;

/// The [`IncidentRecord`] a stored `Incident` node carries, or `None` for a node that
/// is not a served incident.
pub fn incident_record(properties: &[u8]) -> Option<IncidentRecord> {
    let node = medulla_types::msgpack::decode_property_value(properties).ok()?;
    let body = node.get(INCIDENT_RECORD_PROPERTY)?.as_str()?;
    serde_json::from_str(body).ok()
}

/// The `HealthAnomaly` node id of one reading of one incident.
pub fn evidence_id(incident_id: &str, reading: &AnomalyReading) -> String {
    let key = format!(
        "{incident_id}|{}|{}|{}",
        reading.entity_ref, reading.signal, reading.at_ms
    );
    let digest = Digest256::sha256(key.as_bytes()).to_hex();
    format!("anom-{}", &digest[..32])
}

fn stamp(properties: &mut Map<String, Value>, tenant: &str, agent: &str) {
    properties.insert("tenant_id".into(), tenant.into());
    properties.insert("_owner".into(), agent.into());
}

/// One `upsert_node` / `upsert_edge` operation per changed incident, its added
/// evidence nodes, then the edges, all stamped with `tenant` and `agent`. Returns the
/// operations and how many of them are nodes. Refuses `INVALID_ARGUMENT` past
/// [`MAX_INCIDENT_OPERATIONS`].
pub fn incident_operations<'a>(
    records: impl Iterator<Item = (&'a IncidentRecord, &'a [AnomalyReading])>,
    tenant: &str,
    agent: &str,
) -> Result<(Vec<Value>, u64), String> {
    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    for (record, added) in records {
        let body = serde_json::to_string(record)
            .map_err(|_| "incident record encoding failed".to_string())?;
        let mut properties = Map::new();
        properties.insert("type".into(), INCIDENT_TYPE.into());
        properties.insert(INCIDENT_RECORD_PROPERTY.into(), body.into());
        properties.insert("status".into(), json!(record.status));
        properties.insert("severity".into(), json!(record.severity));
        properties.insert(
            "signature_digest".into(),
            record.signature_digest.clone().into(),
        );
        stamp(&mut properties, tenant, agent);
        nodes.push(json!({"op": "upsert_node", "id": record.id, "properties": properties}));
        for reading in added {
            let id = evidence_id(&record.id, reading);
            let mut properties = Map::new();
            properties.insert("type".into(), HEALTH_ANOMALY_TYPE.into());
            properties.insert("entity_ref".into(), reading.entity_ref.clone().into());
            properties.insert("signal".into(), reading.signal.clone().into());
            properties.insert("observed".into(), json!(reading.observed));
            properties.insert("baseline".into(), json!(reading.baseline));
            properties.insert("at_ms".into(), reading.at_ms.into());
            stamp(&mut properties, tenant, agent);
            nodes.push(json!({"op": "upsert_node", "id": id, "properties": properties}));
            let mut edge = Map::from_iter([("relationship".to_string(), HAS_EVIDENCE.into())]);
            stamp(&mut edge, tenant, agent);
            edges.push(json!({
                "op": "upsert_edge",
                "source": record.id,
                "target": id,
                "properties": edge,
            }));
        }
    }
    if nodes.len() + edges.len() > MAX_INCIDENT_OPERATIONS {
        return Err("INVALID_ARGUMENT: the incident evidence exceeds one BatchUpdate".to_string());
    }
    let written = nodes.len() as u64;
    nodes.extend(edges);
    Ok((nodes, written))
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_catalog_types::incident::{IncidentSeverity, IncidentStatus};

    fn record() -> IncidentRecord {
        IncidentRecord {
            id: "inc-1".into(),
            tenant: "acme".into(),
            signature_digest: "0".repeat(64),
            severity: IncidentSeverity::High,
            status: IncidentStatus::Open,
            opened_at_ms: 1,
            evidence: vec![AnomalyReading {
                entity_ref: "svc:a".into(),
                signal: "cpu".into(),
                observed: 300.0,
                baseline: 100.0,
                at_ms: 1,
            }],
            status_history: vec![IncidentStatus::Open],
        }
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R760.3
    #[test]
    fn one_batch_carries_the_incident_and_its_evidence() {
        let record = record();
        let (operations, nodes) = incident_operations(
            std::iter::once((&record, record.evidence.as_slice())),
            "acme",
            "agent-1",
        )
        .unwrap();
        assert_eq!(nodes, 2);
        assert_eq!(operations.len(), 3);
        assert_eq!(operations[0]["properties"]["tenant_id"], "acme");
        let body = operations[0]["properties"][INCIDENT_RECORD_PROPERTY]
            .as_str()
            .unwrap();
        assert_eq!(
            serde_json::from_str::<IncidentRecord>(body).unwrap(),
            record
        );
        assert_eq!(operations[2]["op"], "upsert_edge");
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R760.6
    #[test]
    fn a_status_only_change_rewrites_no_evidence() {
        let record = record();
        let (operations, nodes) =
            incident_operations(std::iter::once((&record, &[][..])), "acme", "agent-1").unwrap();
        assert_eq!(nodes, 1);
        assert_eq!(operations.len(), 1);
        assert_eq!(operations[0]["id"], record.id.as_str());
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R760.3
    #[test]
    fn evidence_ids_are_content_derived() {
        let record = record();
        let a = evidence_id(&record.id, &record.evidence[0]);
        assert_eq!(a, evidence_id(&record.id, &record.evidence[0]));
        assert!(a.starts_with("anom-") && a.len() == 37);
    }
}
