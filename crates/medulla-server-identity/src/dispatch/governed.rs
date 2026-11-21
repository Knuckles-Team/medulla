//! Governed-change dispatch (EH-560).
//!
//! As with elevations, who acted is never read from the body:
//! [`stamp_governed_actor`] overwrites `Method::GovernedChange.actor` from the
//! verified context at the request boundary, before consensus routing; a
//! replicated apply uses its leader's stamp and nothing else.

use medulla_mutation_types::governed_change::GovernedActor;
#[cfg(feature = "security")]
use medulla_mutation_types::governed_change::{GovernedChangeOp, SOURCE_AUTHORITY_KIND};
#[cfg(feature = "security")]
use medulla_protocol_types::{
    result_contract::security::GovernedApprove, result_contract::security::GovernedGet,
    result_contract::security::GovernedList, result_contract::security::GovernedPropose,
    result_contract::security::GovernedRevoke,
};

use super::elevation::ElevationStampAuthority;
use medulla_protocol_types::protocol::Method;
#[cfg(feature = "security")]
use medulla_protocol_types::protocol::Response;
#[cfg(feature = "security")]
use medulla_protocol_types::protocol::ResultPayload;
use medulla_server_api::authority_context::VerifiedRequestContext;

/// The actor of a verified request: its identities (hashed), whether it is
/// delegated, and its EXACT `governance:*` scopes.
pub fn governed_actor(context: &VerifiedRequestContext) -> GovernedActor {
    let claims = context.claims();
    let scopes = claims
        .scopes
        .iter()
        .filter(|scope| scope.starts_with("governance:") && !scope.ends_with(":*"))
        .cloned()
        .collect();
    let identities = std::iter::once(claims.principal.as_str())
        .chain(claims.delegation.iter().map(String::as_str));
    GovernedActor::from_identities(
        &claims.agent_id,
        identities,
        !claims.delegation.is_empty(),
        scopes,
    )
}

/// Overwrite an external governed op's actor from its verified context.
pub fn stamp_governed_actor(
    method: &mut Method,
    context: &VerifiedRequestContext,
    authority: ElevationStampAuthority,
) {
    if authority == ElevationStampAuthority::Replicated {
        return;
    }
    if let Method::GovernedChange { actor, .. } = method {
        *actor = Some(governed_actor(context));
    }
}

/// Serve one governed op against the RBAC policy image.
#[cfg(feature = "security")]
pub async fn dispatch_governed_change(
    state: &std::sync::Arc<tokio::sync::RwLock<medulla_server_api::state::ServerState>>,
    req_id: u64,
    context: &VerifiedRequestContext,
    authority: ElevationStampAuthority,
    (op, stamped): (GovernedChangeOp, Option<GovernedActor>),
    lease_graph: LeaseGraph<'_>,
) -> Response {
    let actor = match (authority, stamped) {
        (ElevationStampAuthority::External, _) => governed_actor(context),
        (ElevationStampAuthority::Replicated, Some(actor)) => actor,
        (ElevationStampAuthority::Replicated, None) => {
            return Response::err(
                req_id,
                "GOVERNED_ACTOR_UNSTAMPED: a replicated governed op must carry its actor",
            )
        }
    };
    let now_ms = medulla_kernel::kernel::dispatch_support::authoritative_now_ms();
    let mut guard = medulla_server_api::state::timed_write(state).await;
    let isolation = &mut guard.isolation;
    let mut approved = None;
    let outcome = match &op {
        GovernedChangeOp::Propose { request } => isolation
            .try_propose_change(&actor, request, now_ms)
            .map(ResultPayload::of::<GovernedPropose>),
        GovernedChangeOp::Approve { request } => isolation
            .try_approve_change(&actor, request, now_ms)
            .map(|change| {
                approved = Some(change.clone());
                ResultPayload::of::<GovernedApprove>(change)
            }),
        GovernedChangeOp::Revoke { change_id } => isolation
            .try_revoke_change(&actor, change_id, now_ms)
            .map(ResultPayload::of::<GovernedRevoke>),
        GovernedChangeOp::Get { change_id } => Ok(ResultPayload::of::<GovernedGet>(
            isolation.rbac().governed().get(change_id, now_ms),
        )),
        GovernedChangeOp::List => Ok(ResultPayload::of::<GovernedList>(
            isolation.rbac().governed().list(now_ms),
        )),
    };
    drop(guard);
    if let (Ok(Ok(_)), Some(change), ElevationStampAuthority::External) =
        (&outcome, &approved, authority)
    {
        if change.kind == SOURCE_AUTHORITY_KIND {
            if let Err(error) =
                issue_approval_lease(state, req_id, context, &lease_graph, change, now_ms).await
            {
                return Response::err(req_id, error);
            }
        }
    }
    match outcome {
        Ok(Ok(payload)) => {
            if op.is_mutation() {
                tracing::info!(
                    target: "medulla::governance::audit",
                    op = op.name(),
                    actor = %actor.agent_id,
                    "governed change transition committed"
                );
            }
            Response::ok(req_id, payload)
        }
        Ok(Err(error)) => Response::err(req_id, error),
        Err(error) => Response::err(req_id, error.to_string()),
    }
}

/// The request graph and carrier of a governed op, for the approval lease an
/// approved source-authority change issues.
#[cfg(feature = "security")]
#[derive(Clone, Copy)]
pub struct LeaseGraph<'a> {
    /// The request graph the lease is issued in.
    pub graph: &'a str,
    /// The agent carrying the request.
    pub caller: Option<&'a str>,
}

/// Approving a source-authority change issues the approval lease bound to its
/// change id (MEDULLA-SRC-AUTH-R003.3): `governed_approval_lease_id`, an
/// `action.approval` lease naming the kind, target, candidate digest and change
/// id, issued and then consumed in the approver's tenant. An issue that finds
/// the lease already there (a retried approval) is not an error.
#[cfg(feature = "security")]
async fn issue_approval_lease(
    state: &std::sync::Arc<tokio::sync::RwLock<medulla_server_api::state::ServerState>>,
    req_id: u64,
    context: &VerifiedRequestContext,
    target: &LeaseGraph<'_>,
    change: &medulla_mutation_types::governed_change::GovernedChange,
    now_ms: u64,
) -> Result<(), String> {
    use medulla_protocol_types::protocol::Method as WireMethod;
    use medulla_work_types::control_lease::{
        ControlLeaseTarget, IssueControlLeaseRequest, TransitionControlLeaseRequest,
    };
    let tenant = context.tenant().to_string();
    let issue = IssueControlLeaseRequest::for_approved_change(&tenant, change, now_ms);
    let lease_id = issue.lease_id.clone();
    let consume = TransitionControlLeaseRequest {
        tenant,
        lease_id: lease_id.clone(),
        // 1 at issue (`ControlLeaseView::revision`).
        expected_revision: 1,
        to: ControlLeaseTarget::Consumed,
        pending_call_consumption: None,
        idempotency_key: format!("{lease_id}:consume"),
    };
    for method in [
        WireMethod::IssueControlLease { request: issue },
        WireMethod::TransitionControlLease { request: consume },
    ] {
        let response = medulla_server_graph::dispatch::graph_pipeline::dispatch_graph_op(
            state,
            target.graph,
            req_id,
            target.caller,
            context,
            method,
        )
        .await;
        if let Some(refusal) = response.refusal_text() {
            return Err(refusal);
        }
    }
    Ok(())
}
