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
    authorize_review, certify, governing, open_change_request, reviewers, transition,
    ReviewRefusal, TargetScopes, CERTIFICATION_TTL_DAYS_DEFAULT,
};
use medulla_protocol_types::protocol::{Method, Response, ResultPayload};
use medulla_protocol_types::result_contract::ingestion::{
    StewardshipApply, StewardshipAssign, StewardshipCertify, StewardshipOpenChangeRequest,
    StewardshipMarkCritical, StewardshipReview, StewardshipRevoke, StewardshipSetTrustFlag,
};
use medulla_server_api::state::ServerState;
use medulla_types::contract::ServerErrorCode;
use tokio::sync::RwLock;

use super::graph_client::{
    encode, read_node, took_effect, write, write_batch, GraphTarget, Properties, TENANT_PROPERTY,
};
use super::shared::typed_response;

/// Node label of an assignment.
pub const ASSIGNMENT_LABEL: &str = "cat:StewardshipAssignment";
/// Node label of a change request.
pub const CHANGE_REQUEST_LABEL: &str = "cat:ChangeRequest";
/// Node label of a certification.
pub const CERTIFICATION_LABEL: &str = "cat:Certification";
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
        StewardshipOp::MarkCritical { .. } => {
            typed_response::<StewardshipMarkCritical>(req_id, result)
        }
        StewardshipOp::SetTrustFlag { .. } => {
            typed_response::<StewardshipSetTrustFlag>(req_id, result)
        }
    }
}

fn invalid(detail: impl std::fmt::Display) -> String {
    format!("INVALID_ARGUMENT: {detail}")
}

fn role_missing(detail: &str) -> String {
    format!("{}: {detail}", ServerErrorCode::StewardRoleMissing.as_str())
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

fn scope_parts(scope: &StewardshipScope) -> (&'static str, &str) {
    match scope {
        StewardshipScope::Asset(id) => ("asset", id),
        StewardshipScope::ConceptScheme(id) => ("concept_scheme", id),
        StewardshipScope::Domain(id) => ("domain", id),
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
            if let Some(assignment) = StewardshipAssignment::from_properties(&properties) {
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
        StewardshipOp::MarkCritical { element } => mark_critical(state, target, element).await,
        StewardshipOp::SetTrustFlag {
            asset,
            flag,
            reason,
        } => set_trust_flag(state, target, asset, flag, reason).await,
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
    let (scope_kind, scope_id) = scope_parts(&assignment.scope);
    let mut properties = Properties::new();
    properties.insert("type".into(), ASSIGNMENT_LABEL.into());
    properties.insert("scope_kind".into(), scope_kind.into());
    properties.insert("scope_id".into(), scope_id.into());
    properties.insert("role".into(), role_key(assignment.role).into());
    properties.insert("principal".into(), assignment.principal.clone().into());
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

pub(super) fn request_id(tenant: &str, asset: &str, digest: &str) -> String {
    format!("change_request:{tenant}:{asset}:{digest}")
}

async fn open(
    state: &Arc<RwLock<ServerState>>,
    target: &GraphTarget<'_>,
    asset: &str,
    proposed_digest: &str,
    scopes: &StewardshipTarget,
) -> Result<StewardshipReceipt, String> {
    let assignments = load_assignments(state, target).await?;
    let scopes = TargetScopes::of_target(scopes);
    let request = open_change_request(asset, proposed_digest, &assignments, &scopes)
        .map_err(|error| role_missing(&error.to_string()))?;
    let (id, request) = persist_request(state, target, asset, proposed_digest, request).await?;
    receipt(state, target, id, &scopes, Some(request), None).await
}

/// Store `request` open under its deterministic id, or return the one already stored there.
async fn persist_request(
    state: &Arc<RwLock<ServerState>>,
    target: &GraphTarget<'_>,
    asset: &str,
    proposed_digest: &str,
    request: ChangeRequest,
) -> Result<(String, ChangeRequest), String> {
    let tenant = target.verified.tenant();
    let id = request_id(tenant, asset, proposed_digest);
    if read_node(state, target, &id).await?.is_some() {
        let stored = stored_request(state, target, &id).await?;
        return Ok((id, stored.request));
    }
    let mut properties = request_properties(tenant, &id, &request, target.verified.agent_id())?;
    properties.insert("verdicts".into(), serde_json::json!([]));
    properties.insert(
        "digest_before".into(),
        target_digest(state, target, asset).await?.into(),
    );
    write(
        state,
        target,
        Method::CreateNodeIfAbsent {
            node_id: id.clone(),
            properties_msgpack: encode(&properties)?,
        },
    )
    .await?;
    Ok((id, request))
}

/// Open the change request a change of `product` (a data product or a described asset) raises
/// (MEDULLA-TYPED-PACKS-R805.3, R801.7). The reviewers are the owners and stewards governing any
/// of `datasets`. A subject none of whose assets is governed raises none: `Ok(None)`.
pub async fn open_governed_change(
    state: &Arc<RwLock<ServerState>>,
    target: &GraphTarget<'_>,
    product: &str,
    proposed_digest: &str,
    datasets: &[String],
) -> Result<Option<String>, String> {
    let assignments = load_assignments(state, target).await?;
    let mut reviewers_of_datasets: Vec<String> = datasets
        .iter()
        .flat_map(|dataset| {
            reviewers(
                &assignments,
                &TargetScopes {
                    asset: Some(dataset.clone()),
                    concept_scheme: None,
                    domain: None,
                },
            )
        })
        .collect();
    reviewers_of_datasets.sort();
    reviewers_of_datasets.dedup();
    if reviewers_of_datasets.is_empty() {
        return Ok(None);
    }
    let request = ChangeRequest {
        target: product.to_string(),
        proposed_digest: proposed_digest.to_string(),
        state: ChangeRequestState::Open,
        reviewers: reviewers_of_datasets,
    };
    request.validate().map_err(invalid)?;
    let (id, _) = persist_request(state, target, product, proposed_digest, request).await?;
    Ok(Some(id))
}

fn request_properties(
    tenant: &str,
    id: &str,
    request: &ChangeRequest,
    opened_by: &str,
) -> Result<Properties, String> {
    let mut properties = Properties::new();
    properties.insert("type".into(), CHANGE_REQUEST_LABEL.into());
    properties.insert("request_id".into(), id.into());
    properties.insert("target".into(), request.target.clone().into());
    properties.insert("state".into(), state_name(request.state).into());
    properties.insert("proposed_digest".into(), request.proposed_digest.clone().into());
    properties.insert("opened_by".into(), opened_by.into());
    properties.insert("reviewers".into(), serde_json::json!(request.reviewers));
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

fn state_from(name: &str) -> Option<ChangeRequestState> {
    match name {
        "open" => Some(ChangeRequestState::Open),
        "approved" => Some(ChangeRequestState::Approved),
        "rejected" => Some(ChangeRequestState::Rejected),
        "applied" => Some(ChangeRequestState::Applied),
        _ => None,
    }
}

/// A change request as stored, with the facts its review checks.
pub(super) struct StoredRequest {
    pub(super) request: ChangeRequest,
    verdicts: Vec<serde_json::Value>,
    opened_by: String,
    /// The target's content digest when the request was opened; empty when it had none.
    digest_before: String,
}

/// The content digest of `asset` now; empty when the asset has none.
async fn target_digest(
    state: &Arc<RwLock<ServerState>>,
    target: &GraphTarget<'_>,
    asset: &str,
) -> Result<String, String> {
    Ok(read_node(state, target, asset)
        .await?
        .and_then(|properties| {
            properties
                .get("content_digest")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_default())
}

pub(super) async fn stored_request(
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
    let text = |key: &str| properties.get(key).and_then(serde_json::Value::as_str);
    let not_a_request = || invalid(format!("node '{id}' is not a change request"));
    let reviewers = properties
        .get("reviewers")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(not_a_request)?
        .iter()
        .filter_map(|reviewer| reviewer.as_str().map(str::to_string))
        .collect();
    let request = ChangeRequest {
        target: text("target").ok_or_else(not_a_request)?.to_string(),
        proposed_digest: text("proposed_digest").ok_or_else(not_a_request)?.to_string(),
        state: state_from(text("state").ok_or_else(not_a_request)?).ok_or_else(not_a_request)?,
        reviewers,
    };
    let verdicts = properties
        .get("verdicts")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();
    Ok(StoredRequest {
        request,
        verdicts,
        opened_by: text("opened_by").unwrap_or_default().to_string(),
        digest_before: text("digest_before").unwrap_or_default().to_string(),
    })
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

/// Check the verified principal against a stored request: the role, self-approval and a target
/// that moved since the request was opened (MEDULLA-TYPED-PACKS-R802.4).
async fn authorize(
    state: &Arc<RwLock<ServerState>>,
    target: &GraphTarget<'_>,
    stored: &StoredRequest,
    approving: bool,
) -> Result<(), String> {
    let current = target_digest(state, target, &stored.request.target).await?;
    authorize_review(
        &stored.request,
        &stored.opened_by,
        target.verified.agent_id(),
        approving,
        &stored.digest_before,
        &current,
    )
    .map_err(|refusal| match refusal {
        ReviewRefusal::RoleMissing => role_missing(&refusal.to_string()),
        ReviewRefusal::SelfApproval => format!(
            "{}: {refusal}",
            ServerErrorCode::SelfApprovalDenied.as_str()
        ),
        ReviewRefusal::Stale { .. } => format!(
            "{}: {refusal}",
            ServerErrorCode::ChangeRequestStale.as_str()
        ),
    })
}

async fn review(
    state: &Arc<RwLock<ServerState>>,
    target: &GraphTarget<'_>,
    id: &str,
    approve: bool,
) -> Result<StewardshipReceipt, String> {
    let stored = stored_request(state, target, id).await?;
    let principal = target.verified.agent_id();
    authorize(state, target, &stored, approve).await?;
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
    authorize(state, target, &stored, false).await?;
    let mut extra = Properties::new();
    extra.insert("digest_before".into(), stored.digest_before.clone().into());
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

/// The target and proposed digest of change request `id` of the verified tenant.
pub async fn request_subject(
    state: &Arc<RwLock<ServerState>>,
    target: &GraphTarget<'_>,
    id: &str,
) -> Result<(String, String), String> {
    let stored = stored_request(state, target, id).await?;
    Ok((stored.request.target, stored.request.proposed_digest))
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

/// Whether the tenant holds an approved quality rule declared on `element`.
async fn has_active_quality_rule(
    state: &Arc<RwLock<ServerState>>,
    target: &GraphTarget<'_>,
    element: &str,
) -> Result<bool, String> {
    use medulla_h_jobs::quality_rule::{QUALITY_RULE_LABEL, RULE_BODY_PROPERTY, RULE_STATE_PROPERTY};
    let tenant = target.verified.tenant();
    let mut after: Option<String> = None;
    loop {
        let response = super::graph_pipeline::dispatch_graph_op(
            state,
            target.graph,
            target.req_id,
            target.caller,
            target.verified,
            Method::GetNodesByLabel {
                label: QUALITY_RULE_LABEL.to_string(),
                after: after.clone(),
                limit: PAGE,
            },
        )
        .await;
        if let Some(refusal) = response.refusal_text() {
            return Err(refusal);
        }
        let Some(ResultPayload::NodeList(nodes)) = response.result else {
            return Ok(false);
        };
        let count = nodes.len();
        after = nodes.last().map(|(id, _)| id.clone());
        for (_, properties) in nodes {
            let text = |key: &str| properties.get(key).and_then(serde_json::Value::as_str);
            let declared_on = text(RULE_BODY_PROPERTY)
                .and_then(|body| serde_json::from_str::<serde_json::Value>(body).ok())
                .and_then(|rule| rule.get("target").and_then(serde_json::Value::as_str).map(str::to_string));
            let approved = properties
                .get(RULE_STATE_PROPERTY)
                .and_then(|state| state.get("state").and_then(serde_json::Value::as_str))
                .or_else(|| text(RULE_STATE_PROPERTY))
                == Some("approved");
            if text(TENANT_PROPERTY) == Some(tenant) && approved && declared_on.as_deref() == Some(element) {
                return Ok(true);
            }
        }
        if count < PAGE {
            return Ok(false);
        }
    }
}

/// Seed the registered catalog stored queries into the graph when it holds none of that name
/// (MEDULLA-TYPED-PACKS-R802.8): the first marking of a critical element in a graph registers
/// them, written in the stored-query node shape `StoredQueryPut` publishes, authored by the
/// marking principal and shared with the tenant. What is seeded is defined once, by
/// `medulla_h_query::query::catalog_query_seed_nodes`, which the first-catalog-write hook
/// (`seed_catalog_queries`) shares.
async fn seed_catalog_queries(
    state: &Arc<RwLock<ServerState>>,
    target: &GraphTarget<'_>,
    author: &str,
    tenant: &str,
) -> Result<(), String> {
    let mut existing = std::collections::BTreeSet::new();
    for head in medulla_h_query::query::catalog_query_head_nodes() {
        if read_node(state, target, &head).await?.is_some() {
            existing.insert(head);
        }
    }
    for (node_id, value) in
        medulla_h_query::query::catalog_query_seed_nodes(author, tenant, &existing)?
    {
        let mut properties = Properties::new();
        if let serde_json::Value::Object(map) = value {
            for (key, field) in map {
                properties.insert(key, field);
            }
        }
        write(
            state,
            target,
            Method::AddNode { node_id, properties_msgpack: encode(&properties)? },
        )
        .await?;
    }
    Ok(())
}

/// Mark `element` critical and record what it misses (MEDULLA-TYPED-PACKS-R802.8). The marker
/// must be an owner or steward of the element; the stored query `catalog.critical_gaps` lists
/// every element whose `gaps` is not empty.
async fn mark_critical(
    state: &Arc<RwLock<ServerState>>,
    target: &GraphTarget<'_>,
    element: &str,
) -> Result<StewardshipReceipt, String> {
    use medulla_source_types::catalog_queries::CRITICAL_ELEMENT_LABEL;
    if element.trim().is_empty() {
        return Err(invalid("a critical element names its term or column"));
    }
    let scopes = TargetScopes {
        asset: Some(element.to_string()),
        concept_scheme: None,
        domain: None,
    };
    let assignments = load_assignments(state, target).await?;
    let principal = target.verified.agent_id();
    if !reviewers(&assignments, &scopes).iter().any(|r| r == principal) {
        return Err(role_missing("marking critical needs the steward or owner role of the element"));
    }
    let (owners, stewards) = governing(&assignments, &scopes);
    let mut gaps = Vec::new();
    if owners.is_empty() {
        gaps.push("owner");
    }
    if stewards.is_empty() {
        gaps.push("steward");
    }
    if !has_active_quality_rule(state, target, element).await? {
        gaps.push("quality_rule");
    }
    let tenant = target.verified.tenant();
    seed_catalog_queries(state, target, principal, tenant).await?;
    let id = format!("critical_element:{tenant}:{element}");
    let mut properties = Properties::new();
    properties.insert("type".into(), CRITICAL_ELEMENT_LABEL.into());
    properties.insert("element".into(), element.into());
    properties.insert("marked_by".into(), principal.into());
    properties.insert("gaps".into(), gaps.join(",").into());
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
    receipt(state, target, id, &scopes, None, None).await
}

/// The trust-flag node of `principal` on `asset`: one per pair, so a new flag replaces the old.
fn trust_flag_id(tenant: &str, asset: &str, principal: &str) -> String {
    format!("trust_flag:{tenant}:{asset}:{principal}")
}

/// The batch a trust flag applies (MEDULLA-TYPED-PACKS-R802.7): the flag node upsert and the
/// asset node upsert whose `flag` is the union of every principal's current flag and the flags
/// the asset already carries that are not trust flags (such as `owner_missing`).
struct TrustFlagPlan {
    id: String,
    operations: Vec<serde_json::Value>,
}

/// Plan one trust flag. `others` are the flags other principals hold on the asset; `asset_node`
/// is the asset's stored properties, absent when the asset has no node of its own yet.
fn plan_trust_flag(
    tenant: &str,
    principal: &str,
    asset: &str,
    flag: &str,
    reason: &str,
    others: &[String],
    asset_node: Option<(String, Properties)>,
) -> Result<TrustFlagPlan, String> {
    use medulla_catalog_types::stewardship::{TRUST_FLAGS, TRUST_FLAG_LABEL};
    if asset.trim().is_empty() {
        return Err(invalid("a trust flag names its asset"));
    }
    if !TRUST_FLAGS.contains(&flag) {
        return Err(invalid(format!(
            "trust flag '{flag}' is not one of endorsement, warning, deprecation"
        )));
    }
    if reason.trim().is_empty() {
        return Err(invalid("a trust flag carries a reason"));
    }
    let id = trust_flag_id(tenant, asset, principal);
    let mut node = Properties::new();
    node.insert("type".into(), TRUST_FLAG_LABEL.into());
    node.insert("asset".into(), asset.into());
    node.insert("principal".into(), principal.into());
    node.insert("flag".into(), flag.into());
    node.insert("reason".into(), reason.into());
    node.insert(TENANT_PROPERTY.into(), tenant.into());
    let mut operations = vec![serde_json::json!({"op": "upsert_node", "id": id, "properties": node})];
    if let Some((asset_key, mut properties)) = asset_node {
        let cat_flag = format!("{}flag", medulla_catalog_types::catalog_relevance::CAT_NS);
        let mut flags: Vec<String> = Vec::new();
        for key in [cat_flag.as_str(), "flag"] {
            match properties.remove(key) {
                Some(serde_json::Value::String(one)) => flags.push(one),
                Some(serde_json::Value::Array(many)) => {
                    flags.extend(many.iter().filter_map(|v| v.as_str().map(str::to_string)))
                }
                _ => {}
            }
        }
        flags.retain(|held| !TRUST_FLAGS.contains(&held.as_str()));
        flags.extend(others.iter().cloned());
        flags.push(flag.to_string());
        flags.sort();
        flags.dedup();
        properties.insert("flag".into(), flags.into());
        operations.push(serde_json::json!({"op": "upsert_node", "id": asset_key, "properties": properties}));
    }
    Ok(TrustFlagPlan { id, operations })
}

/// The flags every other principal of the tenant holds on `asset`.
async fn other_trust_flags(
    state: &Arc<RwLock<ServerState>>,
    target: &GraphTarget<'_>,
    asset: &str,
    principal: &str,
) -> Result<Vec<String>, String> {
    use medulla_catalog_types::stewardship::TRUST_FLAG_LABEL;
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
                label: TRUST_FLAG_LABEL.to_string(),
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
            let text = |key: &str| properties.get(key).and_then(serde_json::Value::as_str);
            if text(TENANT_PROPERTY) == Some(tenant)
                && text("asset") == Some(asset)
                && text("principal") != Some(principal)
            {
                found.extend(text("flag").map(str::to_string));
            }
        }
        if count < PAGE {
            break;
        }
    }
    Ok(found)
}

/// `SetTrustFlag`, plan then apply: the plan is decided from the stored state before any write,
/// and applied as one `BatchUpdate` whose compile derives the `catalog.trust_flag` outbox intent
/// of a warning or deprecation (`trust_flag_intents`), so the flag and its event commit together.
async fn set_trust_flag(
    state: &Arc<RwLock<ServerState>>,
    target: &GraphTarget<'_>,
    asset: &str,
    flag: &str,
    reason: &str,
) -> Result<StewardshipReceipt, String> {
    let tenant = target.verified.tenant();
    let principal = target.verified.agent_id();
    let others = other_trust_flags(state, target, asset, principal).await?;
    // An asset loaded from RDF may be keyed by its bracketed IRI; the flag lands on the node
    // that exists.
    let mut asset_node = None;
    for key in [asset.to_string(), format!("<{asset}>")] {
        if let Some(properties) = read_node(state, target, &key).await? {
            asset_node = Some((key, properties));
            break;
        }
    }
    let plan = plan_trust_flag(tenant, principal, asset, flag, reason, &others, asset_node)?;
    write_batch(state, target, &plan.operations).await?;
    let scopes = TargetScopes {
        asset: Some(asset.to_string()),
        concept_scheme: None,
        domain: None,
    };
    receipt(state, target, plan.id, &scopes, None, None).await
}

#[cfg(test)]
mod trust_flag_tests {
    use super::*;

    // spec: MEDULLA-TYPED-PACKS-R802.7
    #[test]
    fn a_flag_replaces_the_callers_and_keeps_other_flags() {
        let mut asset = Properties::new();
        asset.insert("flag".into(), vec!["owner_missing", "endorsement"].into());
        let plan = plan_trust_flag(
            "t",
            "agent-a",
            "urn:asset:1",
            "deprecation",
            "replaced by v2",
            &["warning".to_string()],
            Some(("urn:asset:1".to_string(), asset)),
        )
        .unwrap();
        assert_eq!(plan.id, "trust_flag:t:urn:asset:1:agent-a");
        assert_eq!(plan.operations.len(), 2);
        assert_eq!(plan.operations[0]["properties"]["type"], "cat:TrustFlag");
        assert_eq!(
            plan.operations[1]["properties"]["flag"],
            serde_json::json!(["deprecation", "owner_missing", "warning"])
        );
    }

    // spec: MEDULLA-TYPED-PACKS-R802.7
    #[test]
    fn an_unknown_flag_or_a_missing_reason_is_refused() {
        let refused = |flag: &str, reason: &str| {
            plan_trust_flag("t", "a", "urn:asset:1", flag, reason, &[], None).is_err()
        };
        assert!(refused("praise", "because"));
        assert!(refused("warning", " "));
        assert!(!refused("warning", "stale"));
    }
}
