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

use medulla_catalog_types::incident::{
    AnomalyReading, IncidentRecord, IncidentRelatedClaim, IncidentStatus, IncidentWriteReceipt,
};
use medulla_graph_core::graph::GraphCore;
use medulla_protocol_types::protocol::{Method, Response};
use medulla_protocol_types::result_contract::ingestion::{
    IncidentCorrelate as IncidentCorrelateResult, IncidentEvidence as IncidentEvidenceResult,
    IncidentRecordAnomaly as IncidentRecordAnomalyResult,
    IncidentRelatedClaims as IncidentRelatedClaimsResult,
    IncidentSetStatus as IncidentSetStatusResult,
};
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_server_api::carrier::GraphReadAuthority;
use medulla_server_api::state::ServerState;
use medulla_server_graph::dispatch::graph_pipeline::dispatch_graph_op;
use medulla_server_graph::dispatch::shared::{timed_read, typed_response};
use medulla_stream::telemetry::incident_authority::{IncidentAuthority, IncidentRefusal};
use serde_json::Value;
use tokio::sync::RwLock;

use medulla_stream::telemetry::incident_nodes::{
    incident_operations, incident_record, INCIDENT_TYPE,
};
/// Claim → entity edge relation `IncidentRelatedClaims` follows.
pub const ABOUT: &str = "about";

const MAX_INCIDENTS_READ: usize = 100_000;
const MAX_SYMBOLS_READ: usize = 1_000_000;
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
        let (loaded, authority) = load(state, &target).await?;
        let tenant = Some(target.verified.tenant());
        let record = authority.evidence(tenant, &incident_id).map_err(refusal)?;
        let limit = limit
            .unwrap_or(DEFAULT_CLAIM_LIMIT)
            .clamp(1, MAX_CLAIM_LIMIT) as usize;
        // Claims come back only through the caller's row-filtered projection.
        let graph = loaded.read.project_core(&loaded.core);
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
        commit(state, &target, &authority, vec![incident_id], Vec::new()).await
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
    let defects = incident_defect_operations(state, target, &authority).await?;
    commit(state, target, &authority, touched, defects).await
}

/// MEDULLA-SDLC-R003.3: an incident whose newly added evidence names a code symbol files one
/// symptom, in the incident's own batch. A frame is an evidence `entity_ref` or `signal`
/// line; it names a symbol when the one frame resolution of `SymbolIndex` finds a `SYMBOL`
/// node of the request graph for it. Evidence the incident already held adds nothing, so a
/// replayed batch files nothing.
async fn incident_defect_operations(
    state: &Arc<RwLock<ServerState>>,
    target: &IncidentTarget<'_>,
    authority: &IncidentAuthority,
) -> Result<Vec<Value>, String> {
    let filings: Vec<(String, AnomalyReading)> = authority
        .changes()
        .flat_map(|(record, added)| {
            added
                .iter()
                .map(move |reading| (record.id.clone(), reading.clone()))
        })
        .collect();
    if filings.is_empty() {
        return Ok(Vec::new());
    }
    let (loaded, _) = load(state, target).await?;
    let graph = loaded.read.project_core(&loaded.core);
    let index = medulla_source_types::stack_frame::SymbolIndex::new(
        graph
            .get_nodes_by_label(
                medulla_source_types::stack_frame::SYMBOL_TYPE,
                MAX_SYMBOLS_READ,
            )
            .into_iter()
            .filter_map(|(id, blob)| {
                let node = medulla_types::msgpack::decode_property_value(&blob).ok()?;
                medulla_source_types::stack_frame::SymbolRef::from_node(&id, &node)
            })
            .collect(),
    );
    if index.is_empty() {
        return Ok(Vec::new());
    }
    let tenant = target.verified.tenant();
    let seam = medulla_server_ops::api::ingest::ingest_seam(state)
        .await
        .ok_or_else(|| "INGEST_SEAM_UNAVAILABLE: the defect book is not installed".to_string())?;
    let mut stored =
        std::collections::BTreeMap::<String, Vec<medulla_source_types::lifecycle::Defect>>::new();
    let mut operations = Vec::new();
    for (incident_id, reading) in filings {
        let Some(symbol) = index
            .resolve_frame(&reading.entity_ref)
            .or_else(|| index.resolve_frame(&reading.signal))
        else {
            continue;
        };
        let request = medulla_source_types::lifecycle::handle_file_defect(
            medulla_source_types::lifecycle::FileDefectRequest {
                repo: target.graph.to_owned(),
                intake: medulla_source_types::lifecycle::DefectIntake::Incident,
                title: format!("incident {} at {}", reading.signal, symbol.qualified_symbol),
                severity: None,
                source_record_id: incident_id,
                observed_at: reading.at_ms,
                text: format!("incident {} at {}", reading.signal, symbol.qualified_symbol),
            },
        )
        .map_err(|e| e.to_string())?;
        let repo = request.repo.clone();
        if !stored.contains_key(&repo) {
            let mut defects: Vec<medulla_source_types::lifecycle::Defect> = loaded
                .core
                .get_nodes_by_label(
                    medulla_source_types::lifecycle::DEFECT_TYPE,
                    MAX_INCIDENTS_READ,
                )
                .into_iter()
                .filter_map(|(_, blob)| {
                    let node = medulla_types::msgpack::decode_property_value(&blob).ok()?;
                    medulla_source_types::lifecycle::defect_from_node(&node, tenant)
                })
                .filter(|defect| defect.repo == repo)
                .collect();
            defects.sort_by(|left, right| left.id.cmp(&right.id));
            stored.insert(repo.clone(), defects);
        }
        let existing = stored.get(&repo).cloned().unwrap_or_default();
        let write = seam.file_defect(existing, request);
        // A second reading of the same batch files against the book this one just changed.
        let book = stored.entry(repo).or_default();
        book.retain(|defect| defect.id != write.defect.id);
        book.push(write.defect.clone());
        book.sort_by(|left, right| left.id.cmp(&right.id));
        let mut properties = write.node_properties()?;
        properties.insert("tenant_id".into(), tenant.into());
        properties.insert("_owner".into(), target.verified.agent_id().into());
        operations.push(serde_json::json!({
            "op": "upsert_node",
            "id": write.node_id(),
            "properties": properties,
        }));
    }
    Ok(operations)
}

/// A refusal as its registered wire code.
fn refusal(error: IncidentRefusal) -> String {
    match &error {
        IncidentRefusal::NotFound(id) => format!("{}: {id}", error.code()),
        IncidentRefusal::Transition(refused) => refused.to_string(),
        IncidentRefusal::InvalidArgument(detail) => format!("{}: {detail}", error.code()),
        _ => error.code().to_string(),
    }
}

/// The authoritative request graph and the caller's row-read authority over it.
struct LoadedGraph {
    core: Arc<GraphCore>,
    read: GraphReadAuthority,
}

/// The request graph and an authority over the verified tenant's unresolved
/// served incidents visible to the caller.
async fn load(
    state: &Arc<RwLock<ServerState>>,
    target: &IncidentTarget<'_>,
) -> Result<(LoadedGraph, IncidentAuthority), String> {
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
    let incidents = IncidentAuthority::load(records);
    Ok((
        LoadedGraph {
            core,
            read: authority,
        },
        incidents,
    ))
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

/// Commit the incidents the call changed and the evidence it added as ONE
/// `BatchUpdate`; `ids` is what the call answers. Evidence already committed
/// is not rewritten, so a replayed batch writes nothing.
async fn commit(
    state: &Arc<RwLock<ServerState>>,
    target: &IncidentTarget<'_>,
    authority: &IncidentAuthority,
    ids: Vec<String>,
    defects: Vec<Value>,
) -> Result<IncidentWriteReceipt, String> {
    let (mut operations, nodes_written) = incident_operations(
        authority.changes(),
        target.verified.tenant(),
        target.verified.agent_id(),
    )?;
    operations.extend(defects);
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
