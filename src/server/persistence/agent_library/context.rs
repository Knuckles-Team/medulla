//! Binding of Agent Library mutation contexts to the verified request.

use medulla_types::contract::Nonce;

#[cfg(feature = "redb")]
/// Bind a request-supplied Agent Library mutation context to the verified request: tenant,
/// principal, attempt nonce, policy digest and idempotency key come from the authenticated
/// request, never from the body.
pub fn bind_agent_library_context(
    store: &super::AgentLibraryStore,
    req_id: u64,
    verified: &crate::server::auth::VerifiedRequestContext,
    mut context: medulla_agent_types::agent_library::AgentLibraryMutationContext,
    purpose_id: &str,
    requires_nonce: bool,
) -> Result<medulla_agent_types::agent_library::AgentLibraryMutationContext, String> {
    if context.tenant_id != verified.tenant() {
        return Err(
            "ACCESS_DENIED: Agent Library tenant must match verified request tenant".to_string(),
        );
    }
    let policy_revision = verified.claims().policy_version.clone();
    let caller = verified.principal_persistence_id();
    let nonce = match verified.attempt_nonce() {
        Some(nonce) => nonce,
        None if requires_nonce => {
            return Err("Agent Library writes require an authenticated attempt nonce".to_string())
        }
        // Status is a read-only query and never reaches the mutation kernel;
        // an internal query producer may still provide an explicit ephemeral
        // nonce to satisfy the shared context shape without consuming it.
        None => Nonce::minted(),
    };
    context.request_id = req_id;
    context.principal = store.owner_principal().to_string();
    context.caller_principal = caller.clone();
    context.attempt_nonce = nonce;
    context.tenant_id = verified.tenant().to_string();
    context.actor_scope = caller;
    // A string, not a record-typed enum: this owner carries agent entries AND
    // agent graphs (RF-ADR-008), and the purpose is the only thing that varies.
    context.purpose_id = purpose_id.to_string();
    context.policy_revision = policy_revision;
    context.policy_digest = super::current_agent_library_policy_digest()?;
    // This identifier is derived from the admitted policy revision, rather
    // than copied from a request body. It stays stable across transport retries
    // so it cannot turn a byte-identical operation into a false conflict.
    context.policy_decision_id = format!("agent-library:policy:{}", context.policy_revision);
    context.idempotency_key = verified.idempotency_key().to_string();
    context.trace_id = None;
    context.created_at_ms = medulla_kernel::kernel::dispatch_support::authoritative_now_ms();
    Ok(context)
}
