//! `ApproveSourceQuery` admission (MEDULLA-FEDERATED-QUERY-R085.2).
//!
//! The caller's connector-pack source-approval receipt is a candidate reference: admission
//! re-verifies it against the registered source and the durable approval, derives the
//! authority from the verified request context (a wire `admission` is overwritten), then
//! commits one lease through the native work-item path. External SQL never runs here.

use std::sync::Arc;

use medulla_graph_core::graph::GraphCore;
use medulla_persistence_records::agent_library::AgentLibraryStateExt;
use medulla_protocol_types::protocol::{Method, Response};
use medulla_result_types::source_query::SourceQueryError;
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_server_api::carrier::CarrierAuthority;
use medulla_server_api::source_query_authority::source_query_authority;
use medulla_server_api::state::ServerState;
use medulla_server_ops::api::query_txn::ForeignStateExt;
use medulla_types::contract::Digest256;
use medulla_work_types::scheduler_config::source_query_knobs;
use medulla_work_types::source_query_approval::{
    approval_refusal, ApproveSourceQueryRequest, SourceQueryAdmission,
};
use tokio::sync::RwLock;

/// Already-authorized graph and placement context for one approval.
pub struct HandleContext<'a> {
    /// The server state that holds the registered sources and the Agent Library.
    pub state: &'a Arc<RwLock<ServerState>>,
    /// The request id.
    pub req_id: u64,
    /// The graph the approval is committed to.
    pub graph_name: &'a str,
    /// The verified caller.
    pub caller: Option<&'a str>,
    /// The verified request context.
    pub verified_context: &'a VerifiedRequestContext,
    /// The graph projection.
    pub core: &'a Arc<GraphCore>,
    /// The persistence backend.
    pub persistence: &'a Option<Arc<dyn medulla_server_api::api::persistence::PersistenceBackend>>,
    /// The routed raft handle.
    #[cfg(feature = "raft")]
    pub routed_raft: &'a Option<medulla_raft::raft::multi::RoutedRaftHandle>,
}

/// Handle `ApproveSourceQuery`; hand any other method back.
pub async fn try_handle(ctx: HandleContext<'_>, method: Method) -> Result<Response, Method> {
    let Method::ApproveSourceQuery {
        mut request,
        source_approval,
    } = method
    else {
        return Err(method);
    };
    if let Err(error) = admit_source_query(&ctx, &mut request, &source_approval).await {
        return Ok(Response::err(ctx.req_id, error));
    }
    let stamped = Method::ApproveSourceQuery {
        request,
        source_approval,
    };
    let work_item_ctx = crate::work_item::HandleContext {
        req_id: ctx.req_id,
        graph_name: ctx.graph_name,
        caller: ctx.caller,
        verified_context: ctx.verified_context,
        core: ctx.core,
        persistence: ctx.persistence,
        #[cfg(feature = "raft")]
        routed_raft: ctx.routed_raft,
    };
    Ok(match crate::work_item::try_handle(work_item_ctx, stamped).await {
        Ok(response) => response,
        Err(_) => Response::err(
            ctx.req_id,
            "ApproveSourceQuery is not served by this build".to_string(),
        ),
    })
}

/// The checks of an approval, in order, before any commit. Every refusal replies in the
/// `SourceQueryFailure` shape of R085.7 (failed step `approval`).
pub async fn admit_source_query(
    ctx: &HandleContext<'_>,
    request: &mut ApproveSourceQueryRequest,
    source_approval: &medulla_catalog_types::connector_pack::ConnectorPackSourceApprovalResult,
) -> Result<(), String> {
    let verified = ctx.verified_context;
    // (1) The verified transport identity replaces the body retry key; the request, its SQL
    // and its limits are validated and clamped to the tenant ceiling.
    request.query.idempotency_key = verified.idempotency_key().to_string();
    request
        .query
        .validate()
        .map_err(|kind| approval_refusal(kind, &request.query))?;
    medulla_federation::federation::validate_source_query_sql(
        request.query.dialect,
        &request.query.sql,
    )
    .map_err(|kind| approval_refusal(kind, &request.query))?;
    request.query.limits = request
        .query
        .limits
        .clamp_to_ceiling(&source_query_knobs().ceiling())
        .map_err(|kind| approval_refusal(kind, &request.query))?;
    // (2) The source is registered and its durable approval still holds.
    let carrier = CarrierAuthority::from_verified(verified)
        .map_err(|_| approval_refusal(SourceQueryError::Unauthorized, &request.query))?;
    {
        let state = ctx.state.read().await;
        let store = state.agent_library();
        let Some(store) = store.as_deref() else {
            return Err(approval_refusal(
                SourceQueryError::VerificationUnavailable,
                &request.query,
            ));
        };
        state
            .foreign_sources()
            .verify_registered_for(&carrier, store, source_approval, &request.query.source_ref)
            .map_err(|_| approval_refusal(SourceQueryError::Unauthorized, &request.query))?;
    }
    // (3) to (5) The request is pinned to the approved contract, revision and snapshot.
    if !request.query.is_pinned_to(
        &source_approval.contract_digest.to_hex(),
        source_approval.committed_version,
        &source_approval.head.pack_digest.to_hex(),
    ) {
        return Err(approval_refusal(SourceQueryError::StaleSource, &request.query));
    }
    // (6) The admission is derived from the verified context, never from the wire.
    let authority = source_query_authority(verified, ctx.graph_name)
        .map_err(|_| approval_refusal(SourceQueryError::Unauthorized, &request.query))?;
    let invalid = || approval_refusal(SourceQueryError::InvalidQuery, &request.query);
    let approval_id = Digest256::framed(
        b"eg/source-query-approval-id/v1",
        &[
            authority.tenant_ref.as_bytes(),
            authority.owner_ref.as_bytes(),
            request.query.idempotency_key.as_bytes(),
        ],
    )
    .map_err(|_| invalid())?;
    request.admission = Some(SourceQueryAdmission {
        approval_id: format!("source-query-approval:{}", approval_id.to_hex()),
        tenant_ref: authority.tenant_ref,
        owner_ref: authority.owner_ref,
        actor_ref: authority.actor_ref,
        authority_digest: authority.authority_digest,
        policy_digest: authority.policy_digest,
        identity_digest: authority.identity_digest,
        approval_ttl_ms: source_query_knobs().approval_ttl_s.saturating_mul(1000),
    });
    Ok(())
}
