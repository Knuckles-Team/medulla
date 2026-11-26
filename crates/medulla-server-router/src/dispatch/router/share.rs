//! Served share grant and revoke (MEDULLA-IDENTITY-R023.13.3).
//!
//! `Method::RbacAdmin` with a `ShareGrant` or `ShareRevoke` op reads the node's
//! committed share keys, decides the next state with `share_transition`, and
//! commits the keys through the same per-graph gateway `ApplyMutation` uses
//! (`dispatch_graph_op`), so the graph's shard writer is the only lock taken and
//! `row_visibility` reads the committed keys. No second visibility store exists.

use super::*;
use medulla_identity_types::acl::RbacAdminOp;
use medulla_isolation::isolation::share::{
    commons_copy_change, share_transition, CommonsCopyChange, ShareActor, ShareOp, ShareState,
    COMMONS_COPY_KEY, GRANTS_KEY, OWNER_ID_KEY, SHARED_SCOPE_KEY,
};

/// The graph that holds the copies a commons share promotes.
pub(super) const COMMONS_GRAPH: &str = "__commons__";

/// The scope that lets a caller change any node's share.
pub(super) const ADMIN_SCOPE: &str = "kg:admin";

pub(super) type Properties = serde_json::Map<String, serde_json::Value>;

fn encode(value: &Properties) -> Result<Vec<u8>, String> {
    rmp_serde::to_vec_named(value)
        .map_err(|error| format!("INVALID_ARGUMENT: share encoding failed: {error}"))
}

/// The share keys of a node, as committed.
pub(super) fn state_of(node_id: &str, properties: &Properties) -> ShareState {
    let text = |key: &str| {
        properties
            .get(key)
            .and_then(serde_json::Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    };
    let mut grants: Vec<String> = properties
        .get(GRANTS_KEY)
        .and_then(serde_json::Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(|value| value.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    grants.sort();
    ShareState {
        node_id: node_id.to_string(),
        owner_id: text(OWNER_ID_KEY),
        shared_scope: text(SHARED_SCOPE_KEY)
            .unwrap_or_else(|| medulla_isolation::isolation::share::SCOPE_PRIVATE.to_string()),
        grants,
        commons_copy: text(COMMONS_COPY_KEY),
    }
}

fn optional_text(value: &Option<String>) -> serde_json::Value {
    value
        .as_ref()
        .map_or(serde_json::Value::Null, |text| serde_json::Value::String(text.clone()))
}

/// The answer for a node that does not exist, which is also the answer for a
/// node the caller cannot see.
pub(super) fn node_absent(node_id: &str) -> String {
    format!("INVALID_ARGUMENT: node '{node_id}' does not exist")
}

/// Whether the actor may see the node: its owner, a `kg:admin` holder, a named
/// grantee, or anyone when the node is shared with the org or the commons.
pub(super) fn visible_to(state: &ShareState, actor: &ShareActor) -> bool {
    use medulla_isolation::isolation::share::{SCOPE_COMMONS, SCOPE_ORG};
    actor.admin
        || actor.delegation.is_some()
        || state.owner_id.as_deref() == Some(actor.principal_id.as_str())
        || state.shared_scope == SCOPE_COMMONS
        || state.shared_scope == SCOPE_ORG
        || state.grants.iter().any(|grantee| *grantee == actor.principal_id)
}

/// Whether a write answer means the write took effect: a failed compare-and-set
/// answers `false`.
fn took_effect(response: &Response) -> bool {
    !matches!(
        &response.result,
        Some(ResultPayload::Bool(false)) | Some(ResultPayload::Json(serde_json::Value::Bool(false)))
    )
}

/// Read one node's committed properties through the graph gateway. `Ok(None)`
/// is an absent node; a refusal (tenant mismatch, access denied) is returned as
/// the refusal text.
pub(super) async fn read_node(
    state: &Arc<RwLock<ServerState>>,
    req: &DispatchHeader,
    verified_context: &VerifiedRequestContext,
    graph: &str,
    node_id: &str,
) -> Result<Option<Properties>, String> {
    let response = dispatch_graph_op(
        state,
        graph,
        req.id,
        req.agent_id.as_deref(),
        verified_context,
        Method::GetNodeProperties {
            node_id: node_id.to_string(),
        },
    )
    .await;
    if let Some(refusal) = response.refusal_text() {
        return Err(refusal);
    }
    match response.result {
        Some(ResultPayload::Raw(bytes)) => medulla_types::msgpack::decode_property_object(&bytes)
            .map(Some)
            .map_err(|_| "INVALID_ARGUMENT: the node properties are malformed".to_string()),
        Some(ResultPayload::Json(serde_json::Value::Object(object))) => Ok(Some(object)),
        _ => Ok(None),
    }
}

async fn write(
    state: &Arc<RwLock<ServerState>>,
    req: &DispatchHeader,
    verified_context: &VerifiedRequestContext,
    graph: &str,
    method: Method,
) -> Result<Response, String> {
    let response = dispatch_graph_op(
        state,
        graph,
        req.id,
        req.agent_id.as_deref(),
        verified_context,
        method,
    )
    .await;
    match response.refusal_text() {
        Some(refusal) => Err(refusal),
        None => Ok(response),
    }
}

/// Serve one `ShareGrant` or `ShareRevoke`. Every refusal is decided before the
/// first write, so a refused share changes nothing.
pub(super) async fn apply_share_op(
    state: &Arc<RwLock<ServerState>>,
    req: &DispatchHeader,
    verified_context: &VerifiedRequestContext,
    op: RbacAdminOp,
) -> Response {
    use medulla_protocol_types::result_contract::security as results;
    let granting = matches!(op, RbacAdminOp::ShareGrant(_));
    match share_op_outcome(state, req, verified_context, op).await {
        Ok(()) if granting => Response::ok(
            req.id,
            ResultPayload::scalar::<results::RbacShareGrant>("share_granted".to_string()),
        ),
        Ok(()) => Response::ok(
            req.id,
            ResultPayload::scalar::<results::RbacShareRevoke>("share_revoked".to_string()),
        ),
        Err(refusal) => Response::err(req.id, refusal),
    }
}

async fn share_op_outcome(
    state: &Arc<RwLock<ServerState>>,
    req: &DispatchHeader,
    verified_context: &VerifiedRequestContext,
    op: RbacAdminOp,
) -> Result<(), String> {
    let share = match op {
        RbacAdminOp::ShareGrant(grant) => ShareOp::Grant(grant),
        RbacAdminOp::ShareRevoke(revoke) => ShareOp::Revoke(revoke),
        _ => return Err("INVALID_ARGUMENT: not a share operation".to_string()),
    };
    let node_id = match &share {
        ShareOp::Grant(grant) => grant.node_id.clone(),
        ShareOp::Revoke(revoke) => revoke.node_id.clone(),
    };
    let graph = req.graph.as_str();
    // A node the caller may not see answers exactly as an absent node does, so a
    // refusal never reveals that a hidden node exists.
    let properties = match read_node(state, req, verified_context, graph, &node_id).await {
        Ok(Some(properties)) => properties,
        Ok(None) => return Err(node_absent(&node_id)),
        Err(refusal) if refusal.starts_with("UNIT_OUT_OF_SCOPE") => {
            return Err(node_absent(&node_id))
        }
        Err(refusal) => return Err(refusal),
    };
    let current = state_of(&node_id, &properties);
    let actor = ShareActor {
        principal_id: verified_context.principal().to_string(),
        admin: verified_context.allows_action(ADMIN_SCOPE),
        delegation: None,
    };
    if !visible_to(&current, &actor) {
        return Err(node_absent(&node_id));
    }
    let next = share_transition(&current, &share, &actor).map_err(|refusal| refusal.to_string())?;

    // No share commits unaudited: the reservation is appended before any write, and
    // a refused reservation refuses the share with the writer's own code.
    let event = ShareEvent::new(
        if matches!(share, ShareOp::Grant(_)) {
            "RbacAdmin.ShareGrant"
        } else {
            "RbacAdmin.ShareRevoke"
        },
        &node_id,
        &current.shared_scope,
        &next.shared_scope,
        &next.grants,
    );
    record_share_event(state, req, verified_context, &event, SharePhase::Reserved).await?;
    match commit_share(state, req, verified_context, &node_id, &properties, &current, &next).await
    {
        Ok(()) => {
            record_share_event(state, req, verified_context, &event, SharePhase::Ok).await?;
            Ok(())
        }
        Err(refusal) => {
            // The refusal stands; a failed audit append cannot change it.
            let _ = record_share_event(state, req, verified_context, &event, SharePhase::Error)
                .await;
            Err(refusal)
        }
    }
}

/// The phase of a share's audit event: the reservation before any write, then the
/// outcome.
#[derive(Clone, Copy)]
pub(super) enum SharePhase {
    Reserved,
    Ok,
    Error,
}

/// The audit-chain event of one served share (MEDULLA-IDENTITY-R023.13.5).
pub(crate) struct ShareEvent {
    pub(crate) op: &'static str,
    pub(crate) node_id: String,
    pub(crate) prior: String,
    pub(crate) next: String,
    pub(crate) grantee_digest: String,
    pub(crate) request_id: String,
    /// The reason the actor was allowed to change the share, when it acted under a
    /// verified product role (MEDULLA-TYPED-PACKS-R805.3); the audit entry records it.
    pub(crate) authority: Option<String>,
}

impl ShareEvent {
    /// The event of a share that moves `node_id` from `prior` to `next` and leaves
    /// `grants` as its grantee list; the digest does not depend on their order.
    pub(crate) fn new(
        op: &'static str,
        node_id: &str,
        prior: &str,
        next: &str,
        grants: &[String],
    ) -> Self {
        Self {
            op,
            node_id: node_id.to_string(),
            prior: prior.to_string(),
            next: next.to_string(),
            grantee_digest: grantee_digest(grants),
            request_id: uuid::Uuid::new_v4().to_string(),
            authority: None,
        }
    }

    /// The same event with the reason the actor was allowed to share.
    pub(crate) fn with_authority(mut self, reason: String) -> Self {
        self.authority = Some(reason);
        self
    }

    /// The operation-audit record of one phase of this share.
    #[cfg(feature = "security")]
    pub(crate) fn audit_record(
        &self,
        tenant: &str,
        principal: &str,
        reservation: bool,
        outcome: medulla_store::redb_store::audit_status::AuditStatus,
    ) -> medulla_store::redb_store::OperationAuditEvent {
        use medulla_store::redb_store::audit_status::AuditStatus;
        medulla_store::redb_store::OperationAuditEvent {
            tenant: tenant.to_string(),
            principal: principal.to_string(),
            op: self.op.to_string(),
            surface: "rbac_admin".to_string(),
            params_sha256: self.grantee_digest.clone(),
            status: if reservation { AuditStatus::Reserved } else { outcome },
            request_id: self.request_id.clone(),
            audit_class: "event".to_string(),
            subject_iri: Some(self.node_id.clone()),
            policy_iri: Some(match &self.authority {
                Some(reason) => format!("{}->{};authority={reason}", self.prior, self.next),
                None => format!("{}->{}", self.prior, self.next),
            }),
            evidence: Default::default(),
        }
    }
}

/// Lowercase-hex SHA-256 over the sorted grantee list joined by newlines (the
/// digest of the empty string when the list is empty).
fn grantee_digest(grants: &[String]) -> String {
    use sha2::{Digest, Sha256};
    let mut sorted = grants.to_vec();
    sorted.sort();
    Sha256::digest(sorted.join("\n").as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Append one phase of a share's `OP_AUDIT` event to the request graph's audit
/// chain. The commit revision of a share is the chain `seq` of its `Ok` entry.
#[cfg(feature = "security")]
pub(super) async fn record_share_event(
    state: &Arc<RwLock<ServerState>>,
    req: &DispatchHeader,
    verified_context: &VerifiedRequestContext,
    event: &ShareEvent,
    phase: SharePhase,
) -> Result<(), String> {
    let authority = medulla_server_api::carrier::CarrierAuthority::from_verified(verified_context)?;
    use medulla_store::redb_store::audit_status::AuditStatus;
    let record = match phase {
        SharePhase::Reserved => {
            event.audit_record(authority.tenant_scope(), authority.actor_scope(), true, AuditStatus::Ok)
        }
        SharePhase::Ok => {
            event.audit_record(authority.tenant_scope(), authority.actor_scope(), false, AuditStatus::Ok)
        }
        SharePhase::Error => {
            event.audit_record(authority.tenant_scope(), authority.actor_scope(), false, AuditStatus::Error)
        }
    };
    let response = medulla_server_graph::dispatch::graph_pipeline::dispatch_audit_append_for_graph(
        state, &req.graph, req.id, record,
    )
    .await;
    match response.refusal_text() {
        Some(refusal) => Err(refusal),
        None => Ok(()),
    }
}

#[cfg(not(feature = "security"))]
pub(super) async fn record_share_event(
    _state: &Arc<RwLock<ServerState>>,
    _req: &DispatchHeader,
    _verified_context: &VerifiedRequestContext,
    _event: &ShareEvent,
    _phase: SharePhase,
) -> Result<(), String> {
    Err("AUDIT_WRITER_UNAVAILABLE: share requires the durable audit chain".to_string())
}

/// Commit an accepted share: the commons copy first on a promotion, then the
/// node's keys, then the copy removal on a revoke.
pub(super) async fn commit_share(
    state: &Arc<RwLock<ServerState>>,
    req: &DispatchHeader,
    verified_context: &VerifiedRequestContext,
    node_id: &str,
    properties: &Properties,
    current: &ShareState,
    next: &ShareState,
) -> Result<(), String> {
    let node_id = node_id.to_string();
    let graph = req.graph.as_str();
    // The commons copy goes first on a promotion, so a refused copy leaves the
    // node untouched; a removal follows the node's own commit.
    let copy_change = commons_copy_change(current, next);
    if let CommonsCopyChange::Create(copy_id) = &copy_change {
        let mut copy = properties.clone();
        copy.insert(
            SHARED_SCOPE_KEY.to_string(),
            serde_json::Value::String(next.shared_scope.clone()),
        );
        copy.remove(GRANTS_KEY);
        copy.remove(COMMONS_COPY_KEY);
        // The copy names its source graph and instant, so the reconcile sweep
        // (R023.13.7) can find the node that must vouch for it.
        copy.insert(
            "_commons_source_graph".to_string(),
            serde_json::Value::String(graph.to_string()),
        );
        copy.insert(
            "_created_at_ms".to_string(),
            serde_json::Value::from(medulla_kernel::kernel::dispatch_support::authoritative_now_ms()),
        );
        write(
            state,
            req,
            verified_context,
            COMMONS_GRAPH,
            Method::AddNode {
                node_id: copy_id.clone(),
                properties_msgpack: encode(&copy)?,
            },
        )
        .await?;
    }

    let mut conditions = Properties::new();
    conditions.insert(
        SHARED_SCOPE_KEY.to_string(),
        properties
            .get(SHARED_SCOPE_KEY)
            .cloned()
            .unwrap_or(serde_json::Value::Null),
    );
    conditions.insert(
        COMMONS_COPY_KEY.to_string(),
        optional_text(&current.commons_copy),
    );
    let mut updates = Properties::new();
    updates.insert(
        SHARED_SCOPE_KEY.to_string(),
        serde_json::Value::String(next.shared_scope.clone()),
    );
    updates.insert(
        GRANTS_KEY.to_string(),
        serde_json::Value::Array(
            next.grants
                .iter()
                .map(|grantee| serde_json::Value::String(grantee.clone()))
                .collect(),
        ),
    );
    updates.insert(COMMONS_COPY_KEY.to_string(), optional_text(&next.commons_copy));
    let committed = write(
        state,
        req,
        verified_context,
        graph,
        Method::CompareAndSetNodeFields {
            node_id: node_id.clone(),
            conditions_msgpack: encode(&conditions)?,
            updates_msgpack: encode(&updates)?,
        },
    )
    .await;
    let stale = || "INVALID_ARGUMENT: the share changed since it was read".to_string();
    let committed = match committed {
        Ok(response) if took_effect(&response) => response,
        other => {
            // Undo the promotion copy: the node never became a commons node.
            if let CommonsCopyChange::Create(copy_id) = &copy_change {
                let _ = write(
                    state,
                    req,
                    verified_context,
                    COMMONS_GRAPH,
                    Method::RemoveNode {
                        node_id: copy_id.clone(),
                    },
                )
                .await;
            }
            return Err(other.err().unwrap_or_else(stale));
        }
    };
    drop(committed);
    if let CommonsCopyChange::Remove(copy_id) = copy_change {
        write(
            state,
            req,
            verified_context,
            COMMONS_GRAPH,
            Method::RemoveNode { node_id: copy_id },
        )
        .await?;
    }
    Ok(())
}
