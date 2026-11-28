//! The served schema drift records and repair approval (MEDULLA-TYPED-PACKS-R806):
//! `SchemaDriftList`, `SchemaDriftGet`, `SchemaRepairPropose` and `SchemaRepairApprove`.
//!
//! List and get read the restricted `SourceQuarantine` nodes the caller can see in the
//! request graph, under the caller's tenant. Propose derives the repair candidate from the
//! stored shape of a drifted stream and runs the shadow check of `GraphSchema ValidateRepair`,
//! which installs nothing. Approve checks the approved digest against the candidate the drift
//! derives and activates it through `GraphSchema AttachApproved`, the one path that verifies
//! the approval receipt and commits the schema source. No second activation route exists.

use super::*;

use std::sync::Arc;

use medulla_catalog_types::graph_schema::approval::approved_candidate_digest;
use medulla_catalog_types::graph_schema::GraphSchemaOp;
use medulla_graph_core::graph::GraphCore;
use medulla_h_ingest::source_ingestion::drift_methods::{
    approved_proposal, get, list, repair_proposal, source_id_of, QuarantineRow,
};
use medulla_protocol_types::result_contract::ingestion::{
    SchemaDriftGet as SchemaDriftGetResult, SchemaDriftList as SchemaDriftListResult,
    SchemaRepairApprove as SchemaRepairApproveResult,
    SchemaRepairPropose as SchemaRepairProposeResult,
};
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_server_api::state::ServerState;
use medulla_server_graph::dispatch::shared::{
    authorized_graph_read, ensure_graph_resident, timed_read, typed_response,
};
use medulla_source_types::schema_drift_methods::{
    SchemaDriftListRequest, SchemaDriftRef, SchemaRepairApproveRequest, SchemaRepairApproved,
    SchemaRepairPreview, SchemaRepairTarget, REPAIR_APPROVAL_MISSING, SOURCE_QUARANTINE_TYPE,
};
use serde_json::{Map, Value};
use tokio::sync::RwLock;

const MAX_QUARANTINES_READ: usize = 100_000;

/// The refusal codes of `AttachApproved` that mean the approval receipt does not authorize
/// this candidate: absent, pending, refused, expired or bound to another candidate.
const APPROVAL_REFUSALS: [&str; 2] = ["SCHEMA_APPROVAL_REQUIRED", "SCHEMA_APPROVAL_MISMATCH"];

fn require_tenant(verified: &VerifiedRequestContext) -> Result<(), String> {
    if verified.tenant().trim().is_empty() {
        return Err("TENANT_SCOPE_REQUIRED".to_string());
    }
    Ok(())
}

/// The caller's view of the request graph: the core and its read authority.
async fn read_graph(
    state: &Arc<RwLock<ServerState>>,
    req: &DispatchHeader,
    verified: &VerifiedRequestContext,
) -> Result<
    (
        Arc<GraphCore>,
        medulla_server_api::carrier::GraphReadAuthority,
    ),
    String,
> {
    ensure_graph_resident(state, &req.graph).await;
    let current = timed_read(state).await;
    authorized_graph_read(
        &current,
        &req.graph,
        verified,
        &format!("GRAPH_NOT_FOUND: {}", req.graph),
    )
}

/// Every quarantine row the caller can see, with its decoded properties.
fn quarantine_rows(
    core: &GraphCore,
    authority: &medulla_server_api::carrier::GraphReadAuthority,
) -> Vec<QuarantineRow> {
    core.get_nodes_by_label(SOURCE_QUARANTINE_TYPE, MAX_QUARANTINES_READ)
        .into_iter()
        .filter(|(id, blob)| authority.can_see_node(blob, core.is_schema_node(id)))
        .filter_map(|(id, blob)| {
            let Ok(Value::Object(node)) = medulla_types::msgpack::decode_property_value(&blob)
            else {
                return None;
            };
            Some((id, node))
        })
        .collect::<Vec<(String, Map<String, Value>)>>()
}

pub async fn drift_list(
    state: &Arc<RwLock<ServerState>>,
    req: &DispatchHeader,
    verified: &VerifiedRequestContext,
    request: SchemaDriftListRequest,
) -> Response {
    let result = async {
        require_tenant(verified)?;
        let (core, authority) = read_graph(state, req, verified).await?;
        let rows = quarantine_rows(&core, &authority);
        Ok(list(&rows, verified.tenant(), request.stream.as_ref()))
    }
    .await;
    typed_response::<SchemaDriftListResult>(req.id, result)
}

pub async fn drift_get(
    state: &Arc<RwLock<ServerState>>,
    req: &DispatchHeader,
    verified: &VerifiedRequestContext,
    request: SchemaDriftRef,
) -> Response {
    let result = async {
        require_tenant(verified)?;
        let (core, authority) = read_graph(state, req, verified).await?;
        let rows = quarantine_rows(&core, &authority);
        get(&rows, verified.tenant(), &request.drift_id)
    }
    .await;
    typed_response::<SchemaDriftGetResult>(req.id, result)
}

/// The composed schema digest an inner `GraphSchema` answer reports.
fn composed_digest_of(response: &Response) -> Result<String, String> {
    match &response.result {
        Some(medulla_protocol_types::protocol::ResultPayload::Json(value)) => value
            .get("composed_digest")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| {
                "INVALID_ARGUMENT: the schema answer names no composed digest".to_string()
            }),
        _ => Err("INVALID_ARGUMENT: the schema gateway answered no result".to_string()),
    }
}

/// Run one schema operation through the graph gateway as the caller.
async fn schema_op(
    state: &Arc<RwLock<ServerState>>,
    req: &DispatchHeader,
    verified: &VerifiedRequestContext,
    op: GraphSchemaOp,
) -> Result<Response, String> {
    let response = dispatch_boxed(dispatch_graph_op(
        state,
        &req.graph,
        req.id,
        req.agent_id.as_deref(),
        verified,
        Method::GraphSchema { op: Box::new(op) },
    ))
    .await;
    match response.refusal_text() {
        Some(refusal) => Err(refusal),
        None => Ok(response),
    }
}

pub async fn repair_propose(
    state: &Arc<RwLock<ServerState>>,
    req: &DispatchHeader,
    verified: &VerifiedRequestContext,
    target: SchemaRepairTarget,
) -> Response {
    let result = async {
        require_tenant(verified)?;
        let proposal = {
            let (core, authority) = read_graph(state, req, verified).await?;
            let rows = quarantine_rows(&core, &authority);
            repair_proposal(&rows, verified.tenant(), &target)?
        };
        let response = schema_op(
            state,
            req,
            verified,
            GraphSchemaOp::ValidateRepair {
                source_id: proposal.source_id.clone(),
                contract: proposal.candidate.clone(),
                if_composed_digest: None,
            },
        )
        .await?;
        // The shadow check stored its report on the graph in its own commit: read its counts.
        let shadow_check = {
            let (core, _) = read_graph(state, req, verified).await?;
            medulla_h_admin::graph_schema::shadow_check_summary(&core, &proposal.candidate_digest)
                .ok_or_else(|| {
                "INVALID_ARGUMENT: the shadow check stored no report for the candidate".to_string()
            })?
        };
        Ok(SchemaRepairPreview {
            drift_id: target.drift_id,
            stream: target.stream,
            source_id: proposal.source_id,
            candidate: proposal.candidate,
            candidate_digest: proposal.candidate_digest,
            composed_digest: composed_digest_of(&response)?,
            shadow_check,
            applied: false,
        })
    }
    .await;
    typed_response::<SchemaRepairProposeResult>(req.id, result)
}

pub async fn repair_approve(
    state: &Arc<RwLock<ServerState>>,
    req: &DispatchHeader,
    verified: &VerifiedRequestContext,
    request: SchemaRepairApproveRequest,
) -> Response {
    let result = async {
        require_tenant(verified)?;
        request.validate()?;
        let target = SchemaRepairTarget {
            drift_id: request.drift_id.clone(),
            stream: request.stream.clone(),
        };
        let (proposal, in_force, composed) = {
            let (core, authority) = read_graph(state, req, verified).await?;
            let rows = quarantine_rows(&core, &authority);
            let proposal =
                approved_proposal(&rows, verified.tenant(), &target, &request.proposal_digest)?;
            let sources = core.schema_sources();
            let in_force = sources
                .dynamic
                .get(&source_id_of(&request.stream))
                .and_then(|source| source.contract.as_ref())
                .is_some_and(|contract| {
                    approved_candidate_digest(&proposal.source_id, contract)
                        == proposal.candidate_digest
                });
            (proposal, in_force, sources.composed_digest().to_hex())
        };
        // The candidate is already the contract in force: a second approval activates nothing.
        if in_force {
            return Ok(SchemaRepairApproved {
                drift_id: request.drift_id,
                stream: request.stream,
                candidate_digest: proposal.candidate_digest,
                activated: false,
                composed_digest: composed,
            });
        }
        let response = schema_op(
            state,
            req,
            verified,
            GraphSchemaOp::AttachApproved {
                source_id: proposal.source_id.clone(),
                contract: proposal.candidate,
                approval_lease_id: request.approval_lease_id,
                expected_digest: None,
                if_composed_digest: None,
            },
        )
        .await
        .map_err(|refusal| {
            if APPROVAL_REFUSALS
                .iter()
                .any(|code| refusal.starts_with(code))
            {
                format!("{REPAIR_APPROVAL_MISSING}: {refusal}")
            } else {
                refusal
            }
        })?;
        Ok(SchemaRepairApproved {
            drift_id: request.drift_id,
            stream: request.stream,
            candidate_digest: proposal.candidate_digest,
            activated: true,
            composed_digest: composed_digest_of(&response)?,
        })
    }
    .await;
    typed_response::<SchemaRepairApproveResult>(req.id, result)
}
