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
    // The approval lease is issued BEFORE the approval transition and the
    // transition is refused when the issue fails, so an approved source-authority
    // change always has its lease (MEDULLA-SRC-AUTH-R003.3).
    #[cfg(feature = "security")]
    if let (GovernedChangeOp::Approve { request }, ElevationStampAuthority::External) =
        (&op, authority)
    {
        if let Err(error) = issue_approval_lease_for_pending(
            state,
            req_id,
            context,
            &lease_graph,
            &request.change_id,
            now_ms,
        )
        .await
        {
            return Response::err(req_id, error);
        }
    }
    let mut guard = medulla_server_api::state::timed_write(state).await;
    let isolation = &mut guard.isolation;
    let outcome = match &op {
        GovernedChangeOp::Propose { request } => isolation
            .try_propose_change(&actor, request, now_ms)
            .map(ResultPayload::of::<GovernedPropose>),
        GovernedChangeOp::Approve { request } => isolation
            .try_approve_change(&actor, request, now_ms)
            .map(ResultPayload::of::<GovernedApprove>),
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

/// Approving a source-authority change first issues the approval lease bound to
/// its change id (MEDULLA-SRC-AUTH-R003.3): `governed_approval_lease_id`, an
/// `action.approval` lease naming the kind, target, candidate digest and change
/// id, left `active` for the one attach that spends it. A change of another kind
/// (or none) issues nothing. An issue that finds the lease already there (a
/// retried approval) is not an error. The lease alone attaches nothing: the
/// attach also needs the change `approved` in the governed ledger.
#[cfg(feature = "security")]
async fn issue_approval_lease_for_pending(
    state: &std::sync::Arc<tokio::sync::RwLock<medulla_server_api::state::ServerState>>,
    req_id: u64,
    context: &VerifiedRequestContext,
    target: &LeaseGraph<'_>,
    change_id: &str,
    now_ms: u64,
) -> Result<(), String> {
    use medulla_protocol_types::protocol::Method as WireMethod;
    use medulla_work_types::control_lease::IssueControlLeaseRequest;
    let change = state
        .read()
        .await
        .isolation
        .rbac()
        .governed()
        .get(change_id, now_ms);
    let Some(change) = change.filter(|change| change.kind == SOURCE_AUTHORITY_KIND) else {
        return Ok(());
    };
    let issue = IssueControlLeaseRequest::for_approved_change(
        &context.tenant().to_string(),
        &change,
        now_ms,
    );
    let response = medulla_server_graph::dispatch::graph_pipeline::dispatch_graph_op(
        state,
        target.graph,
        req_id,
        target.caller,
        context,
        WireMethod::IssueControlLease { request: issue },
    )
    .await;
    match response.refusal_text() {
        Some(refusal) => Err(refusal),
        None => Ok(()),
    }
}
