//! `Method::Stewardship`: the served stewardship store and change-request flow
//! (MEDULLA-TYPED-PACKS-R802.3).
//!
//! Assignments, change requests and certifications are graph nodes of the verified
//! tenant, written through the ordinary graph gateway (`dispatch_graph_op`), so each
//! write is ACL-checked, durable, audited and CDC-emitted. The rules are the pure
//! functions of `medulla_core::stewardship`; this module reads the stored
//! assignments, decides every refusal before the first write and commits.
//!
//! * `Assign` and `Revoke` upsert and remove one `StewardshipAssignment` node.
//! * `OpenChangeRequest` resolves the reviewers of the target from the stored
//!   assignments (`open_change_request`) and stores the request `open`.
//! * `Review` needs a resolved reviewer: it moves `open` to `approved`, or to
//!   `rejected` on a rejection, and records the verdict.
//! * `Apply` needs a resolved reviewer and an `approved` request: it moves it to
//!   `applied` and records the digests and the receipt id.
//! * `Certify` needs a steward or owner of the asset.
//!
//! Every answer carries the owners and stewards governing the target.

use std::sync::Arc;

use medulla_catalog_types::stewardship::{
    ChangeRequest, ChangeRequestState, StewardRole, StewardshipAssignment, StewardshipOp,
    StewardshipReceipt, StewardshipScope, StewardshipTarget,
};
use medulla_core::stewardship::{
    certify, open_change_request, resolve, reviewers, transition, TargetScopes,
    CERTIFICATION_TTL_DAYS_DEFAULT,
};
use medulla_protocol_types::protocol::{Method, Response, ResultPayload};
use medulla_protocol_types::result_contract::ingestion::{
    StewardshipApply, StewardshipAssign, StewardshipCertify, StewardshipOpenChangeRequest,
    StewardshipReview, StewardshipRevoke,
};
use medulla_server_api::state::ServerState;
use medulla_types::contract::ServerErrorCode;
use tokio::sync::RwLock;

use super::graph_client::{
    encode, read_node, took_effect, write, GraphTarget, Properties, TENANT_PROPERTY,
};
use super::shared::typed_response;

/// Node label of an assignment.
pub const ASSIGNMENT_LABEL: &str = "StewardshipAssignment";
/// Node label of a change request.
pub const CHANGE_REQUEST_LABEL: &str = "ChangeRequest";
/// Node label of a certification.
pub const CERTIFICATION_LABEL: &str = "Certification";
const RECORD: &str = "record";
const PAGE: usize = 1000;

/// Serve one `Stewardship` op.
pub async fn dispatch_stewardship(
    state: &Arc<RwLock<ServerState>>,
    target: GraphTarget<'_>,
    op: StewardshipOp,
) -> Response {
    let req_id = target.req_id;
    let result = serve(state, &target, &op).await;
    match op {
        StewardshipOp::Assign { .. } => typed_response::<StewardshipAssign>(req_id, result),
        StewardshipOp::Revoke { .. } => typed_response::<StewardshipRevoke>(req_id, result),
        StewardshipOp::OpenChangeRequest { .. } => {
            typed_response::<StewardshipOpenChangeRequest>(req_id, result)
        }
        StewardshipOp::Review { .. } => typed_response::<StewardshipReview>(req_id, result),
        StewardshipOp::Apply { .. } => typed_response::<StewardshipApply>(req_id, result),
        StewardshipOp::Certify { .. } => typed_response::<StewardshipCertify>(req_id, result),
    }
}

fn invalid(detail: impl std::fmt::Display) -> String {
    format!("INVALID_ARGUMENT: {detail}")
}

fn role_missing(detail: &str) -> String {
    format!("{}: {detail}", ServerErrorCode::StewardRoleMissing.as_str())
}

fn scopes_of(target: &StewardshipTarget) -> TargetScopes {
    TargetScopes {
        asset: target.asset.clone(),
        concept_scheme: target.concept_scheme.clone(),
        domain: target.domain.clone(),
    }
}

fn scopes_of_assignment(assignment: &StewardshipAssignment) -> TargetScopes {
    let mut scopes = TargetScopes {
        asset: None,
        concept_scheme: None,
        domain: None,
    };
    match &assignment.scope {
        StewardshipScope::Asset(id) => scopes.asset = Some(id.clone()),
        StewardshipScope::ConceptScheme(id) => scopes.concept_scheme = Some(id.clone()),
        StewardshipScope::Domain(id) => scopes.domain = Some(id.clone()),
    }
    scopes
}

fn scope_key(scope: &StewardshipScope) -> String {
    match scope {
        StewardshipScope::Asset(id) => format!("asset:{id}"),
        StewardshipScope::ConceptScheme(id) => format!("concept_scheme:{id}"),
        StewardshipScope::Domain(id) => format!("domain:{id}"),
    }
}

fn role_key(role: StewardRole) -> &'static str {
    match role {
        StewardRole::Owner => "owner",
        StewardRole::Steward => "steward",
        StewardRole::Custodian => "custodian",
    }
}

fn assignment_id(tenant: &str, assignment: &StewardshipAssignment) -> String {
    format!(
        "stewardship:{tenant}:{}:{}:{}",
        scope_key(&assignment.scope),
        role_key(assignment.role),
        assignment.principal
    )
}

/// Every stewardship assignment of the verified tenant, read from the store.
pub async fn load_assignments(
    state: &Arc<RwLock<ServerState>>,
    target: &GraphTarget<'_>,
) -> Result<Vec<StewardshipAssignment>, String> {
    let tenant = target.verified.tenant();
    let mut found = Vec::new();
    let mut after: Option<String> = None;
    loop {
        let response = super::graph_pipeline::dispatch_graph_op(
            state,
            target.graph,
            target.req_id,
            target.caller,
            target.verified,
            Method::GetNodesByLabel {
                label: ASSIGNMENT_LABEL.to_string(),
                after: after.clone(),
                limit: PAGE,
            },
        )
        .await;
        if let Some(refusal) = response.refusal_text() {
            return Err(refusal);
        }
        let Some(ResultPayload::NodeList(nodes)) = response.result else {
            break;
        };
        let count = nodes.len();
        after = nodes.last().map(|(id, _)| id.clone());
        for (_, properties) in nodes {
            if properties.get(TENANT_PROPERTY).and_then(serde_json::Value::as_str) != Some(tenant)
            {
                continue;
            }
            let Some(body) = properties.get(RECORD).and_then(serde_json::Value::as_str) else {
                continue;
            };
            if let Ok(assignment) = serde_json::from_str::<StewardshipAssignment>(body) {
                found.push(assignment);
            }
        }
        if count < PAGE {
            break;
        }
    }
    Ok(found)
}

/// Whether the verified principal is an owner or steward governing `scopes`, by the
/// stored assignments.
pub async fn holds_review_role(
    state: &Arc<RwLock<ServerState>>,
    target: &GraphTarget<'_>,
    scopes: &TargetScopes,
) -> Result<bool, String> {
    let assignments = load_assignments(state, target).await?;
    let principal = target.verified.agent_id();
    Ok(reviewers(&assignments, scopes)
        .iter()
        .any(|reviewer| reviewer == principal))
}

fn governing(assignments: &[StewardshipAssignment], scopes: &TargetScopes) -> (Vec<String>, Vec<String>) {
    let pick = |role: StewardRole| {
        let mut names: Vec<String> = resolve(assignments, scopes)
            .into_iter()
            .filter(|a| a.role == role)
            .map(|a| a.principal.clone())
            .collect();
        names.sort();
        names.dedup();
        names
    };
    (pick(StewardRole::Owner), pick(StewardRole::Steward))
}

async fn receipt(
    state: &Arc<RwLock<ServerState>>,
    target: &GraphTarget<'_>,
    id: String,
    scopes: &TargetScopes,
    change_request: Option<ChangeRequest>,
    certification: Option<medulla_catalog_types::stewardship::Certification>,
) -> Result<StewardshipReceipt, String> {
    let assignments = load_assignments(state, target).await?;
    let (owners, stewards) = governing(&assignments, scopes);
    Ok(StewardshipReceipt {
        id,
        change_request,
        certification,
        owners,
        stewards,
    })
}

async fn serve(
    state: &Arc<RwLock<ServerState>>,
    target: &GraphTarget<'_>,
    op: &StewardshipOp,
) -> Result<StewardshipReceipt, String> {
    if target.verified.tenant().trim().is_empty() {
        return Err(ServerErrorCode::TenantScopeRequired.as_str().to_string());
    }
    match op {
        StewardshipOp::Assign { assignment } => assign(state, target, assignment).await,
        StewardshipOp::Revoke { assignment } => revoke(state, target, assignment).await,
        StewardshipOp::OpenChangeRequest {
            target: asset,
            proposed_digest,
            scopes,
        } => open(state, target, asset, proposed_digest, scopes).await,
        StewardshipOp::Review {
            request_id,
            approve,
        } => review(state, target, request_id, *approve).await,
        StewardshipOp::Apply { request_id } => apply(state, target, request_id).await,
        StewardshipOp::Certify { asset, level } => certify_asset(state, target, asset, level).await,
    }
}

async fn assign(
    state: &Arc<RwLock<ServerState>>,
    target: &GraphTarget<'_>,
    assignment: &StewardshipAssignment,
) -> Result<StewardshipReceipt, String> {
    assignment.validate().map_err(invalid)?;
    let tenant = target.verified.tenant();
    let id = assignment_id(tenant, assignment);
    let record = serde_json::to_string(assignment).map_err(invalid)?;
    let mut properties = Properties::new();
    properties.insert("type".into(), ASSIGNMENT_LABEL.into());
    properties.insert("scope".into(), scope_key(&assignment.scope).into());
    properties.insert("role".into(), role_key(assignment.role).into());
    properties.insert("principal".into(), assignment.principal.clone().into());
    properties.insert(RECORD.into(), record.into());
    properties.insert(TENANT_PROPERTY.into(), tenant.into());
    write(
        state,
        target,
        Method::AddNode {
            node_id: id.clone(),
            properties_msgpack: encode(&properties)?,
        },
    )
    .await?;
    receipt(state, target, id, &scopes_of_assignment(assignment), None, None).await
}

async fn revoke(
    state: &Arc<RwLock<ServerState>>,
    target: &GraphTarget<'_>,
    assignment: &StewardshipAssignment,
) -> Result<StewardshipReceipt, String> {
    assignment.validate().map_err(invalid)?;
    let id = assignment_id(target.verified.tenant(), assignment);
    if read_node(state, target, &id).await?.is_none() {
        return Err(invalid(format!("assignment '{id}' does not exist")));
    }
    write(state, target, Method::RemoveNode { node_id: id.clone() }).await?;
    receipt(state, target, id, &scopes_of_assignment(assignment), None, None).await
}

fn request_id(tenant: &str, asset: &str, digest: &str) -> String {
    format!("change_request:{tenant}:{asset}:{digest}")
}

async fn open(
    state: &Arc<RwLock<ServerState>>,
    target: &GraphTarget<'_>,
    asset: &str,
    proposed_digest: &str,
    scopes: &StewardshipTarget,
) -> Result<StewardshipReceipt, String> {
    let tenant = target.verified.tenant();
    let assignments = load_assignments(state, target).await?;
    let scopes = scopes_of(scopes);
    let request = open_change_request(asset, proposed_digest, &assignments, &scopes)
        .map_err(|error| role_missing(&error.to_string()))?;
    let id = request_id(tenant, asset, proposed_digest);
    if read_node(state, target, &id).await?.is_some() {
        let stored = stored_request(state, target, &id).await?;
        return receipt(state, target, id, &scopes, Some(stored.request), None).await;
    }
    let mut properties = request_properties(tenant, &id, &request, target.verified.agent_id())?;
    properties.insert("verdicts".into(), serde_json::json!([]));
    write(
        state,
        target,
        Method::CreateNodeIfAbsent {
            node_id: id.clone(),
            properties_msgpack: encode(&properties)?,
        },
    )
    .await?;
    receipt(state, target, id, &scopes, Some(request), None).await
}

fn request_properties(
    tenant: &str,
    id: &str,
    request: &ChangeRequest,
    opened_by: &str,
) -> Result<Properties, String> {
    let record = serde_json::to_string(request).map_err(invalid)?;
    let mut properties = Properties::new();
    properties.insert("type".into(), CHANGE_REQUEST_LABEL.into());
    properties.insert("request_id".into(), id.into());
    properties.insert("target".into(), request.target.clone().into());
    properties.insert("state".into(), state_name(request.state).into());
    properties.insert("proposed_digest".into(), request.proposed_digest.clone().into());
    properties.insert("opened_by".into(), opened_by.into());
    properties.insert(RECORD.into(), record.into());
    properties.insert(TENANT_PROPERTY.into(), tenant.into());
    Ok(properties)
}

fn state_name(state: ChangeRequestState) -> &'static str {
    match state {
        ChangeRequestState::Open => "open",
        ChangeRequestState::Approved => "approved",
        ChangeRequestState::Rejected => "rejected",
        ChangeRequestState::Applied => "applied",
    }
}

struct StoredRequest {
    request: ChangeRequest,
    verdicts: Vec<serde_json::Value>,
}

async fn stored_request(
    state: &Arc<RwLock<ServerState>>,
    target: &GraphTarget<'_>,
    id: &str,
) -> Result<StoredRequest, String> {
    let properties = read_node(state, target, id)
        .await?
        .ok_or_else(|| invalid(format!("change request '{id}' does not exist")))?;
    if properties.get(TENANT_PROPERTY).and_then(serde_json::Value::as_str)
        != Some(target.verified.tenant())
    {
        return Err(invalid(format!("change request '{id}' belongs to another tenant")));
    }
    let body = properties
        .get(RECORD)
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| invalid(format!("node '{id}' is not a change request")))?;
    let request: ChangeRequest = serde_json::from_str(body).map_err(invalid)?;
    let verdicts = properties
        .get("verdicts")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();
    Ok(StoredRequest { request, verdicts })
}

/// Move a stored request to `to`, recording `extra` fields, as a compare-and-set on
/// its current state.
async fn move_request(
    state: &Arc<RwLock<ServerState>>,
    target: &GraphTarget<'_>,
    id: &str,
    stored: &StoredRequest,
    to: ChangeRequestState,
    extra: Properties,
) -> Result<ChangeRequest, String> {
    let next = transition(stored.request.state, to).map_err(|error| invalid(error))?;
    let mut request = stored.request.clone();
    request.state = next;
    let mut conditions = Properties::new();
    conditions.insert("state".into(), state_name(stored.request.state).into());
    conditions.insert(TENANT_PROPERTY.into(), target.verified.tenant().into());
    let mut updates = extra;
    updates.insert("state".into(), state_name(next).into());
    updates.insert(
        RECORD.into(),
        serde_json::to_string(&request).map_err(invalid)?.into(),
    );
    let committed = write(
        state,
        target,
        Method::CompareAndSetNodeFields {
            node_id: id.to_string(),
            conditions_msgpack: encode(&conditions)?,
            updates_msgpack: encode(&updates)?,
        },
    )
    .await?;
    if !took_effect(&committed) {
        return Err(invalid(format!("change request '{id}' changed since it was read")));
    }
    Ok(request)
}

fn require_reviewer(request: &ChangeRequest, principal: &str) -> Result<(), String> {
    if request.reviewers.iter().any(|reviewer| reviewer == principal) {
        Ok(())
    } else {
        Err(role_missing("the principal is not an owner or steward of the target"))
    }
}

async fn review(
    state: &Arc<RwLock<ServerState>>,
    target: &GraphTarget<'_>,
    id: &str,
    approve: bool,
) -> Result<StewardshipReceipt, String> {
    let stored = stored_request(state, target, id).await?;
    let principal = target.verified.agent_id();
    require_reviewer(&stored.request, principal)?;
    let mut verdicts = stored.verdicts.clone();
    verdicts.push(serde_json::json!({"principal": principal, "approve": approve}));
    let mut extra = Properties::new();
    extra.insert("verdicts".into(), serde_json::Value::Array(verdicts));
    let to = if approve {
        ChangeRequestState::Approved
    } else {
        ChangeRequestState::Rejected
    };
    let request = move_request(state, target, id, &stored, to, extra).await?;
    let scopes = TargetScopes {
        asset: Some(request.target.clone()),
        concept_scheme: None,
        domain: None,
    };
    receipt(state, target, id.to_string(), &scopes, Some(request), None).await
}

async fn apply(
    state: &Arc<RwLock<ServerState>>,
    target: &GraphTarget<'_>,
    id: &str,
) -> Result<StewardshipReceipt, String> {
    let stored = stored_request(state, target, id).await?;
    require_reviewer(&stored.request, target.verified.agent_id())?;
    let before = read_node(state, target, &stored.request.target)
        .await?
        .and_then(|properties| properties.get("content_digest").cloned())
        .unwrap_or(serde_json::Value::Null);
    let mut extra = Properties::new();
    extra.insert("digest_before".into(), before);
    extra.insert(
        "digest_after".into(),
        stored.request.proposed_digest.clone().into(),
    );
    extra.insert("receipt_id".into(), format!("receipt:{id}").into());
    extra.insert("applied_by".into(), target.verified.agent_id().into());
    let request = move_request(state, target, id, &stored, ChangeRequestState::Applied, extra).await?;
    let scopes = TargetScopes {
        asset: Some(request.target.clone()),
        concept_scheme: None,
        domain: None,
    };
    receipt(state, target, id.to_string(), &scopes, Some(request), None).await
}

async fn certify_asset(
    state: &Arc<RwLock<ServerState>>,
    target: &GraphTarget<'_>,
    asset: &str,
    level: &str,
) -> Result<StewardshipReceipt, String> {
    let scopes = TargetScopes {
        asset: Some(asset.to_string()),
        concept_scheme: None,
        domain: None,
    };
    if !holds_review_role(state, target, &scopes).await? {
        return Err(role_missing("certifying needs the steward or owner role of the asset"));
    }
    let now_secs = medulla_kernel::kernel::dispatch_support::authoritative_now_ms() / 1000;
    let certification = certify(
        asset,
        level,
        target.verified.agent_id(),
        now_secs,
        CERTIFICATION_TTL_DAYS_DEFAULT,
    )
    .map_err(invalid)?;
    let tenant = target.verified.tenant();
    let id = format!("certification:{tenant}:{asset}");
    let mut properties = Properties::new();
    properties.insert("type".into(), CERTIFICATION_LABEL.into());
    properties.insert("asset".into(), asset.into());
    properties.insert("level".into(), level.into());
    properties.insert("by".into(), certification.by.clone().into());
    properties.insert("expires_at".into(), certification.expires_at.into());
    properties.insert(RECORD.into(), serde_json::to_string(&certification).map_err(invalid)?.into());
    properties.insert(TENANT_PROPERTY.into(), tenant.into());
    write(
        state,
        target,
        Method::AddNode {
            node_id: id.clone(),
            properties_msgpack: encode(&properties)?,
        },
    )
    .await?;
    receipt(state, target, id, &scopes, None, Some(certification)).await
}
