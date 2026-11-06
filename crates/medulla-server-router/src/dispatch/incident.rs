//! The five served incident methods (MEDULLA-UNIFIED-DATA-PLANE-R760.3).
//!
//! Each handler reads the verified tenant from the request carrier (never
//! from the body), loads that tenant's unresolved `Incident` nodes of the
//! request graph into a fresh [`IncidentAuthority`], and applies the one
//! call. A write commits the changed incidents and their `HealthAnomaly`
//! evidence nodes as ONE `BatchUpdate` through the ordinary graph gateway
//! (`dispatch_graph_op`), the same path `TelemetryDerive` writes through, so
//! the write is ACL-checked, durable, audited and CDC-emitted. A read-only
//! call commits nothing.

use std::sync::Arc;

use medulla_graph_core::graph::GraphCore;
use medulla_protocol_types::protocol::{Method, Response};
use medulla_protocol_types::result_contract::ingestion::{
    IncidentCorrelate as IncidentCorrelateResult, IncidentEvidence as IncidentEvidenceResult,
    IncidentRecordAnomaly as IncidentRecordAnomalyResult,
    IncidentRelatedClaims as IncidentRelatedClaimsResult,
    IncidentSetStatus as IncidentSetStatusResult,
};
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_server_api::state::ServerState;
use medulla_server_graph::dispatch::graph_pipeline::dispatch_graph_op;
use medulla_server_graph::dispatch::shared::{timed_read, typed_response};
use medulla_stream::telemetry::incident_authority::{IncidentAuthority, IncidentRefusal};
use medulla_types::incident::{
    AnomalyReading, IncidentRecord, IncidentRelatedClaim, IncidentStatus, IncidentWriteReceipt,
};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use tokio::sync::RwLock;

/// The node type of a served incident.
pub const INCIDENT_TYPE: &str = "Incident";
/// The node type of one evidence reading.
pub const HEALTH_ANOMALY_TYPE: &str = "HealthAnomaly";
/// The property holding the canonical JSON of the [`IncidentRecord`]; an
/// `Incident` node without it (a CEP-derived fact) is not a served incident.
pub const INCIDENT_RECORD_PROPERTY: &str = "incident_record";
/// Incident → evidence edge relation.
pub const HAS_EVIDENCE: &str = "hasEvidence";
/// Claim → entity edge relation `IncidentRelatedClaims` follows.
pub const ABOUT: &str = "about";

const MAX_INCIDENTS_READ: usize = 100_000;
const MAX_BATCH_OPERATIONS: usize = 50_000;
const DEFAULT_CLAIM_LIMIT: u32 = 100;
const MAX_CLAIM_LIMIT: u32 = 1_000;

/// Who asked, and which graph holds the incidents.
pub struct IncidentTarget<'a> {
    pub graph: &'a str,
    pub req_id: u64,
    pub caller: Option<&'a str>,
    pub verified: &'a VerifiedRequestContext,
}

/// `IncidentRecordAnomaly`: record one reading and correlate it.
pub async fn handle_incident_record_anomaly(
    state: &Arc<RwLock<ServerState>>,
    target: IncidentTarget<'_>,
    anomaly: AnomalyReading,
    window_ms: u64,
) -> Response {
    let result = correlate(state, &target, vec![anomaly], window_ms).await;
    typed_response::<IncidentRecordAnomalyResult>(target.req_id, result)
}

/// `IncidentCorrelate`: correlate a set of readings in one pass.
pub async fn handle_incident_correlate(
    state: &Arc<RwLock<ServerState>>,
    target: IncidentTarget<'_>,
    anomalies: Vec<AnomalyReading>,
    window_ms: u64,
) -> Response {
    let result = correlate(state, &target, anomalies, window_ms).await;
    typed_response::<IncidentCorrelateResult>(target.req_id, result)
}

/// `IncidentEvidence`: one incident of the caller's tenant. Read-only.
pub async fn handle_incident_evidence(
    state: &Arc<RwLock<ServerState>>,
    target: IncidentTarget<'_>,
    incident_id: String,
) -> Response {
    let result = async {
        let (_, authority) = load(state, &target).await?;
        let tenant = Some(target.verified.tenant());
        authority
            .evidence(tenant, &incident_id)
            .cloned()
            .map_err(refusal)
    }
    .await;
    typed_response::<IncidentEvidenceResult>(target.req_id, result)
}

/// `IncidentRelatedClaims`: the `Claim` nodes with an `about` edge to an
/// entity in the incident's evidence. Read-only.
pub async fn handle_incident_related_claims(
    state: &Arc<RwLock<ServerState>>,
    target: IncidentTarget<'_>,
    incident_id: String,
    limit: Option<u32>,
) -> Response {
    let result = async {
        let (graph, authority) = load(state, &target).await?;
        let tenant = Some(target.verified.tenant());
        let record = authority.evidence(tenant, &incident_id).map_err(refusal)?;
        let limit = limit
            .unwrap_or(DEFAULT_CLAIM_LIMIT)
            .clamp(1, MAX_CLAIM_LIMIT) as usize;
        Ok(related_claims(&graph, record, limit))
    }
    .await;
    typed_response::<IncidentRelatedClaimsResult>(target.req_id, result)
}

/// `IncidentSetStatus`: move an incident one status forward.
pub async fn handle_incident_set_status(
    state: &Arc<RwLock<ServerState>>,
    target: IncidentTarget<'_>,
    incident_id: String,
    status: IncidentStatus,
) -> Response {
    let result = async {
        let (_, mut authority) = load(state, &target).await?;
        let tenant = Some(target.verified.tenant());
        authority
            .set_status(tenant, &incident_id, status)
            .map_err(refusal)?;
        commit(state, &target, &authority, vec![incident_id]).await
    }
    .await;
    typed_response::<IncidentSetStatusResult>(target.req_id, result)
}

async fn correlate(
    state: &Arc<RwLock<ServerState>>,
    target: &IncidentTarget<'_>,
    anomalies: Vec<AnomalyReading>,
    window_ms: u64,
) -> Result<IncidentWriteReceipt, String> {
    let (_, mut authority) = load(state, target).await?;
    let tenant = Some(target.verified.tenant());
    let touched = authority
        .correlate(tenant, anomalies, window_ms)
        .map_err(refusal)?;
    commit(state, target, &authority, touched).await
}

/// A refusal as its registered wire code.
fn refusal(error: IncidentRefusal) -> String {
    match &error {
        IncidentRefusal::NotFound(id) => format!("{}: {id}", error.code()),
        _ => error.code().to_string(),
    }
}

/// The request graph and an authority over the verified tenant's unresolved
/// served incidents visible to the caller.
async fn load(
    state: &Arc<RwLock<ServerState>>,
    target: &IncidentTarget<'_>,
) -> Result<(Arc<GraphCore>, IncidentAuthority), String> {
    let tenant = target.verified.tenant();
    if tenant.trim().is_empty() {
        return Err(IncidentRefusal::TenantScopeRequired.code().to_string());
    }
    medulla_server_graph::dispatch::shared::ensure_graph_resident(state, target.graph).await;
    let (core, authority) = {
        let current = timed_read(state).await;
        medulla_server_graph::dispatch::shared::authorized_graph_read(
            &current,
            target.graph,
            target.verified,
            &format!("GRAPH_NOT_FOUND: {}", target.graph),
        )?
    };
    let records = core
        .get_nodes_by_label(INCIDENT_TYPE, MAX_INCIDENTS_READ)
        .into_iter()
        .filter(|(node_id, properties)| {
            authority.can_see_node(properties, core.is_schema_node(node_id))
        })
        .filter_map(|(_, properties)| incident_record(&properties))
        .filter(|record| record.tenant == tenant && record.status != IncidentStatus::Resolved);
    Ok((core.clone(), IncidentAuthority::load(records)))
}

fn incident_record(properties: &[u8]) -> Option<IncidentRecord> {
    let node = medulla_types::msgpack::decode_property_value(properties).ok()?;
    let body = node.get(INCIDENT_RECORD_PROPERTY)?.as_str()?;
    serde_json::from_str(body).ok()
}

fn related_claims(
    graph: &GraphCore,
    record: &IncidentRecord,
    limit: usize,
) -> Vec<IncidentRelatedClaim> {
    let mut entities: Vec<&str> = record
        .evidence
        .iter()
        .map(|a| a.entity_ref.as_str())
        .collect();
    entities.sort_unstable();
    entities.dedup();
    let mut out = Vec::new();
    for entity in entities {
        for claim_id in graph.get_predecessors(entity).unwrap_or_default() {
            if out.len() >= limit {
                return out;
            }
            let Some(properties) = graph
                .get_node_properties(&claim_id)
                .and_then(|p| medulla_types::msgpack::decode_property_value(&p).ok())
            else {
                continue;
            };
            let is_claim = properties.get("type").and_then(Value::as_str) == Some("Claim");
            let same_tenant =
                properties.get("tenant_id").and_then(Value::as_str) == Some(record.tenant.as_str());
            if is_claim && same_tenant && is_about(graph, &claim_id, entity) {
                out.push(IncidentRelatedClaim {
                    claim_id,
                    entity_ref: entity.to_string(),
                    properties,
                });
            }
        }
    }
    out
}

fn is_about(graph: &GraphCore, source: &str, target: &str) -> bool {
    graph
        .get_edge_properties(source, target)
        .iter()
        .filter_map(|p| medulla_types::msgpack::decode_property_value(p).ok())
        .any(|edge| edge.get("relationship").and_then(Value::as_str) == Some(ABOUT))
}

fn hex_digest(text: &str) -> String {
    let digest = Sha256::digest(text.as_bytes());
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// The `HealthAnomaly` node id of one reading of one incident.
fn evidence_id(incident_id: &str, reading: &AnomalyReading) -> String {
    let key = format!(
        "{incident_id}|{}|{}|{}",
        reading.entity_ref, reading.signal, reading.at_ms
    );
    format!("anom-{}", &hex_digest(&key)[..32])
}

/// Commit every incident in `ids` and its evidence as ONE `BatchUpdate`.
async fn commit(
    state: &Arc<RwLock<ServerState>>,
    target: &IncidentTarget<'_>,
    authority: &IncidentAuthority,
    ids: Vec<String>,
) -> Result<IncidentWriteReceipt, String> {
    let (operations, nodes_written) = incident_operations(
        authority.records(&ids),
        target.verified.tenant(),
        target.verified.agent_id(),
    )?;
    if !operations.is_empty() {
        let operations_msgpack = rmp_serde::to_vec_named(&operations)
            .map_err(|_| "incident batch encoding failed".to_string())?;
        let response = dispatch_graph_op(
            state,
            target.graph,
            target.req_id,
            target.caller,
            target.verified,
            Method::BatchUpdate { operations_msgpack },
        )
        .await;
        if let Some(refusal) = response.refusal_text() {
            return Err(refusal);
        }
    }
    Ok(IncidentWriteReceipt {
        incident_ids: ids,
        nodes_written,
    })
}

fn stamp(properties: &mut Map<String, Value>, tenant: &str, agent: &str) {
    properties.insert("tenant_id".into(), tenant.into());
    properties.insert("_owner".into(), agent.into());
}

/// Upserts for each incident node, its evidence nodes, then the edges.
fn incident_operations<'a>(
    records: impl Iterator<Item = &'a IncidentRecord>,
    tenant: &str,
    agent: &str,
) -> Result<(Vec<Value>, u64), String> {
    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    for record in records {
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
        for reading in &record.evidence {
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
    if nodes.len() + edges.len() > MAX_BATCH_OPERATIONS {
        return Err("INVALID_ARGUMENT: the incident evidence exceeds one BatchUpdate".to_string());
    }
    let written = nodes.len() as u64;
    nodes.extend(edges);
    Ok((nodes, written))
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_types::incident::{IncidentSeverity, IncidentStatus};

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
        let (operations, nodes) =
            incident_operations(std::iter::once(&record), "acme", "agent-1").unwrap();
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
}
