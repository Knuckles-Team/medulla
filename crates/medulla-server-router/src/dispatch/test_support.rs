//! Request-boundary fixtures shared by the identity store and role-elevation
//! dispatch tests: a verified context with explicit scopes and delegation,
//! and one dispatch through the real boundary.

use std::sync::Arc;

use tokio::sync::RwLock;

use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_server_api::state::ServerState;
use medulla_types::acl::RequestContextClaims;
use medulla_protocol_types::protocol::Method;
use medulla_protocol_types::protocol::Request;
use medulla_protocol_types::protocol::Response;

/// A verified context for `principal` (also the effective agent unless a
/// delegation chain is given).
pub fn verified(
    principal: &str,
    scopes: &[&str],
    delegation: &[&str],
) -> VerifiedRequestContext {
    let agent = delegation.last().copied().unwrap_or(principal);
    verified_as(principal, agent, scopes, delegation)
}

/// [`verified`], with the effective agent named explicitly rather than
/// derived from `principal`/`delegation` -- for a caller whose own principal
/// naming convention (e.g. a `"user:"`-prefixed test principal) would
/// otherwise leak into the agent id.
pub fn verified_as(
    principal: &str,
    agent: &str,
    scopes: &[&str],
    delegation: &[&str],
) -> VerifiedRequestContext {
    static KEY: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let key = KEY.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    VerifiedRequestContext::from_verified_claims(
        RequestContextClaims {
            principal: principal.to_string(),
            tenant: "tenant-shared".to_string(),
            audience: "medulla-test".to_string(),
            agent_id: agent.to_string(),
            roles: Vec::new(),
            scopes: scopes.iter().map(|scope| scope.to_string()).collect(),
            policy_version: "policy-test".to_string(),
            delegation: delegation.iter().map(|hop| hop.to_string()).collect(),
            node: None,
            priority: None,
        },
        format!("dispatch-test-{key}"),
    )
}

/// Dispatch `method` as `context` through the real request boundary.
pub async fn send(
    state: &Arc<RwLock<ServerState>>,
    context: VerifiedRequestContext,
    method: Method,
) -> Response {
    let request = Request {
        id: 11,
        graph: "__commons__".to_string(),
        auth_token: String::new(),
        agent_id: Some(context.agent_id().to_string()),
        method,
    };
    Box::pin(super::request_boundary::dispatch_with_context(
        state,
        request,
        Some(context),
    ))
    .await
}

/// An engine with an empty authorization image.
pub fn state() -> Arc<RwLock<ServerState>> {
    Arc::new(RwLock::new(new_state_for_test(
        "dispatch-test-secret",
        medulla_isolation::isolation::IsolationLayer::new(),
    )))
}

/// An engine whose System identity already bootstrapped: the identity store
/// may hold real principals (IDM ordering).
pub fn bootstrapped_state() -> Arc<RwLock<ServerState>> {
    let mut layer = medulla_isolation::isolation::IsolationLayer::new();
    layer
        .try_bootstrap_system_identity(medulla_types::acl::AgentIdentity {
            agent_id: "engine-root".to_string(),
            role: medulla_types::acl::AgentRole::System,
            teams: Vec::new(),
            roles: Vec::new(),
        })
        .expect("a fresh layer bootstraps its System identity");
    Arc::new(RwLock::new(new_state_for_test(
        "dispatch-test-secret",
        layer,
    )))
}

/// Whether `response` was refused with `code`.
pub fn refused_with(response: &Response, code: &str) -> bool {
    response
        .error
        .as_deref()
        .is_some_and(|error| error.contains(code))
}

/// A test state carrying the `DispatchSeam`, so lower-crate re-entry reaches the real router.
pub fn new_state_for_test(
    auth_secret: impl Into<String>,
    isolation: medulla_isolation::isolation::IsolationLayer,
) -> ServerState {
    medulla_server::seams_install::new_state_for_test(auth_secret, isolation)
}
