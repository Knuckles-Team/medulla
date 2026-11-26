//! Delegated acting identity at the dispatch chokepoint (MEDULLA-IDENTITY-R744).
//!
//! A signed envelope that carries a `delegation_id` claim asks to run under an agent-run
//! delegation of its tenant. The engine verifies the claim here, once, before the request
//! reaches the method gate, and replaces the context with one that runs as the delegation's
//! principal and holds only the delegation's operations. Every later check
//! (`check_scope_and_admin_authority`, graph access, carriers) therefore sees the principal and
//! the bounded scopes, and nothing outside the delegation passes.
//!
//! * The signing host must hold the service-only scope `identity:delegate`; a bearer of someone
//!   else's delegation id gains nothing without it.
//! * The delegation is read tenant-keyed on the engine clock. An unknown id, another tenant's
//!   record, another delegation kind, a revoked or expired record, an inactive principal and a
//!   scope the principal no longer holds all refuse the same `DELEGATION_INVALID`, so the claim
//!   is no existence oracle.
//! * The host is recorded: the context keeps it as its acting host, and one operation audit event
//!   names the host beside the principal it acted for, queued on the request graph's audit chain.
//!   A queue that cannot take it refuses `AUDIT_UNAVAILABLE` before the request runs.

use super::*;

/// The refusal for every delegation that cannot be used, whatever the reason.
const DELEGATION_UNUSABLE: &str = "DELEGATION_INVALID: the agent-run delegation is not active";

/// The scope a host holds to act under a delegation of its tenant.
pub const DELEGATE_SCOPE: &str = "identity:delegate";

/// Replace `context` with the context its delegation claim runs under, or return it unchanged
/// when it claims none.
pub async fn resolve_delegated_actor(
    state: &Arc<RwLock<ServerState>>,
    req: &Request,
    context: VerifiedRequestContext,
) -> Result<VerifiedRequestContext, Response> {
    let Some(delegation_id) = context.delegation_claim().map(str::to_owned) else {
        return Ok(context);
    };
    if !context.allows_method(DELEGATE_SCOPE, true) {
        medulla_kernel::metrics::process().access_denied();
        return Err(Response::err(
            req.id,
            format!(
                "ACCESS_DENIED: verified request context lacks required scope '{DELEGATE_SCOPE}'"
            ),
        ));
    }
    let Some(actor) = lookup(state, context.tenant(), &delegation_id).await else {
        return Err(Response::err(req.id, DELEGATION_UNUSABLE));
    };
    let Some(acting) =
        context.acting_under_delegation(&actor.principal, actor.roles, actor.operations)
    else {
        return Err(Response::err(req.id, DELEGATION_UNUSABLE));
    };
    audit_acting(state, req, &acting, &delegation_id)
        .await
        .map_err(|code| {
            Response::err(
                req.id,
                format!("{}: the delegated request cannot be audited", code.as_str()),
            )
        })?;
    Ok(acting)
}

/// The actor `delegation_id` names for `tenant` right now, on the engine clock.
#[cfg(feature = "security")]
async fn lookup(
    state: &Arc<RwLock<ServerState>>,
    tenant: &str,
    delegation_id: &str,
) -> Option<medulla_identity_types::identity::DelegatedActor> {
    let now_ms = medulla_kernel::kernel::dispatch_support::authoritative_now_ms();
    timed_read(state)
        .await
        .isolation
        .rbac()
        .identity_store()
        .delegated_actor(
            delegation_id,
            tenant,
            now_ms,
            &medulla_capabilities::scopes::ScopeRegistry,
        )
}

/// A build without the identity store holds no delegation record, so no claim resolves.
#[cfg(not(feature = "security"))]
async fn lookup(
    _state: &Arc<RwLock<ServerState>>,
    _tenant: &str,
    _delegation_id: &str,
) -> Option<DelegatedActorStub> {
    None
}

#[cfg(not(feature = "security"))]
struct DelegatedActorStub {
    principal: String,
    roles: Vec<String>,
    operations: Vec<String>,
}

/// The operation audit event of one delegated request: the principal it acted for, the host
/// that signed it and the delegation, never a raw id (the delegation is its digest).
#[cfg(feature = "redb")]
fn acting_event(
    req: &Request,
    acting: &VerifiedRequestContext,
    delegation_id: &str,
) -> Option<medulla_store::redb_store::OperationAuditEvent> {
    use sha2::{Digest, Sha256};
    let host = acting.acting_host()?;
    let method: &'static str = (&req.method).into();
    let host_scope =
        medulla_server_api::mutation_batch::identity_metadata::principal_persistence_id(
            &host.principal,
        );
    let delegation = format!("{:x}", Sha256::digest(delegation_id.as_bytes()));
    let mut evidence = std::collections::BTreeMap::new();
    evidence.insert("acting_host".to_string(), host_scope);
    evidence.insert("delegation_sha256".to_string(), delegation.clone());
    evidence.insert("method".to_string(), method.to_string());
    let attempt = format!(
        "{:x}",
        Sha256::digest(format!("{}\0{}", req.id, acting.idempotency_key()).as_bytes())
    );
    Some(medulla_store::redb_store::OperationAuditEvent {
        tenant: acting.tenant().to_string(),
        principal: acting.principal_persistence_id(),
        op: "delegated_acting".to_string(),
        surface: "dispatch".to_string(),
        params_sha256: format!(
            "{:x}",
            Sha256::digest(format!("{delegation}\0{method}").as_bytes())
        ),
        status: medulla_store::redb_store::audit_status::AuditStatus::settled(
            medulla_store::redb_store::audit_machine::AuditEvent::Succeed,
        ),
        request_id: format!("delegated-{attempt}"),
        audit_class: "event".to_string(),
        subject_iri: None,
        policy_iri: None,
        evidence,
    })
}

/// Queue the audit event of a delegated request. A request the queue cannot take refuses
/// `AUDIT_UNAVAILABLE`, so no delegated request runs unaudited.
#[cfg(feature = "redb")]
async fn audit_acting(
    state: &Arc<RwLock<ServerState>>,
    req: &Request,
    acting: &VerifiedRequestContext,
    delegation_id: &str,
) -> Result<(), medulla_types::contract::ServerErrorCode> {
    use medulla_types::contract::ServerErrorCode;
    let unavailable = ServerErrorCode::AuditUnavailable;
    let event = acting_event(req, acting, delegation_id).ok_or(unavailable)?;
    event.validate().map_err(|_| unavailable)?;
    // A deployment without durable persistence holds no audit chain; the read audit, which
    // this queue is, passes it through the same way.
    let Some(persistence) = timed_read(state).await.persistence.clone() else {
        return Ok(());
    };
    let chain = medulla_store::redb_store::read_audit_chain_scope(
        &event.tenant,
        &medulla_kernel::persist::sanitize(&req.graph),
    );
    medulla_server_api::read_audit::ReadAuditSink::global().enqueue(Box::pin(async move {
        use medulla_persistence::AsRedb;
        match persistence.as_redb() {
            Some(redb) => redb.audit_append(&chain, event).await.map(|_| ()),
            None => Err("AUDIT_WRITER_UNAVAILABLE: no durable audit writer".to_string()),
        }
    }))
}

/// Without the durable audit writer no delegated request can be audited, so none runs.
#[cfg(not(feature = "redb"))]
async fn audit_acting(
    _state: &Arc<RwLock<ServerState>>,
    _req: &Request,
    _acting: &VerifiedRequestContext,
    _delegation_id: &str,
) -> Result<(), medulla_types::contract::ServerErrorCode> {
    Err(medulla_types::contract::ServerErrorCode::AuditUnavailable)
}
