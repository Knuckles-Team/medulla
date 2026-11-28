//! Served participant reads (MEDULLA-DECISION-ENGINE-R655.4).
//!
//! `Method::ParticipantRead` is a graph read: the handler reads the projected core under the
//! verified tenant and principal and commits nothing. One `match` over [`ParticipantReadOp`] picks
//! the resolver. Every refusal of an absent participant, a participant of another tenant or one the
//! caller does not act as is the same `PARTICIPANT_NOT_VISIBLE` envelope, so a reader cannot tell
//! which of the three it hit.

use super::*;
use medulla_decision_types::participant_memory::{DerivationStatus, ParticipantReadOp};
use medulla_h_graph_gateway::graph_ops::ops_context::GraphOpsContext;
use medulla_kernel::kernel::dispatch_support::authoritative_now_ms;
use medulla_protocol_types::result_contract::coordination::{
    ParticipantReadDeriveInput, ParticipantReadGet, ParticipantReadList, ParticipantReadStatus,
};
use medulla_work_types::memory_scope::ScopeRef;
use std::collections::BTreeMap;
use {
    medulla_h_graph_gateway::graph_ops::participant_surface::participant_unavailable,
    medulla_h_graph_gateway::graph_ops::participant_surface::readable_participants,
    medulla_h_graph_gateway::graph_ops::participant_surface::resolve_scope_ref,
    medulla_h_graph_gateway::graph_ops::participant_surface::sessions_visible_to,
};

fn refuse(code: &str, detail: impl std::fmt::Display) -> String {
    format!("{code}: {detail}")
}

/// Route `Method::ParticipantRead`.
pub(super) async fn try_handle_participant_read(
    ctx: GraphOpsContext<'_>,
    method: Method,
) -> ControlFlow<Response, Method> {
    let Method::ParticipantRead { op } = &method else {
        return ControlFlow::Continue(method);
    };
    let Some(tenant) = ctx
        .read_authority
        .carrier()
        .map(|carrier| carrier.tenant_scope().to_string())
    else {
        return ControlFlow::Break(Response::err(
            ctx.req_id,
            refuse(
                "ACCESS_DENIED",
                "a participant read needs a verified tenant",
            ),
        ));
    };
    let agent_id = match ctx.read_authority.verified_actor() {
        Ok(actor) => actor.to_string(),
        Err(error) => return ControlFlow::Break(Response::err(ctx.req_id, error)),
    };
    let answer = serve(&ctx, &tenant, &agent_id, op).await;
    ControlFlow::Break(match answer {
        Ok(payload) => Response::ok(ctx.req_id, payload),
        Err(error) => Response::err(ctx.req_id, error),
    })
}

/// Resolve one operation to its typed answer.
async fn serve(
    ctx: &GraphOpsContext<'_>,
    tenant: &str,
    agent_id: &str,
    op: &ParticipantReadOp,
) -> Result<ResultPayload, String> {
    match op {
        ParticipantReadOp::Get { participant_id } => {
            readable_participants(ctx.core, tenant, agent_id)
                .into_iter()
                .find(|participant| &participant.participant_id == participant_id)
                .ok_or_else(|| participant_unavailable(participant_id.as_str()))
                .and_then(ResultPayload::of::<ParticipantReadGet>)
        }
        ParticipantReadOp::List => ResultPayload::of::<ParticipantReadList>(readable_participants(
            ctx.core, tenant, agent_id,
        )),
        ParticipantReadOp::DeriveInput { payload_ref } => {
            medulla_h_graph_gateway::graph_ops::participant_derive::derive_input(
                ctx.core,
                tenant,
                payload_ref,
            )
            .and_then(ResultPayload::of::<ParticipantReadDeriveInput>)
        }
        ParticipantReadOp::Status { memory_scope } => {
            status(ctx, tenant, agent_id, memory_scope.as_ref())
                .await
                .and_then(ResultPayload::of::<ParticipantReadStatus>)
        }
        // A question and a card read the claim store, which the decide handler holds; the router
        // answers them before a method reaches this chain.
        ParticipantReadOp::Ask { .. } | ParticipantReadOp::Card { .. } => Err(refuse(
            "INTERNAL",
            "ask and card are served by the decide handler, not the graph read chain",
        )),
    }
}

/// The derivation backlog the caller may see: for each scope, session and subject, how many tasks
/// of queue `memory.derive` still wait for a worker and how long the oldest has waited since its
/// newest turn. The sessions are the ones of the participants the caller acts as, narrowed by the
/// memory scope when one is named.
async fn status(
    ctx: &GraphOpsContext<'_>,
    tenant: &str,
    agent_id: &str,
    scope: Option<&ScopeRef>,
) -> Result<Vec<DerivationStatus>, String> {
    let mut sessions = sessions_visible_to(ctx.core, tenant, agent_id);
    if let Some(reference) = scope {
        sessions = resolve_scope_ref(ctx.core, tenant, agent_id, reference, &sessions)?;
    }
    let jobs = medulla_server_ops::api::ingest::jobs_seam(ctx.state)
        .await
        .ok_or_else(|| refuse("ENGINE_UNAVAILABLE", "the derive queue is not installed"))?;
    let persist_dir = { ctx.state.read().await.persist_dir.clone() };
    let now_ms = authoritative_now_ms();
    let mut groups: BTreeMap<(String, String, String), (u32, u64)> = BTreeMap::new();
    for payload in
        medulla_h_graph_gateway::graph_ops::participant_derive::stored_payloads(ctx.core, tenant)
    {
        let key = &payload.key;
        if !sessions.contains(&key.session_id)
            || !jobs.task_pending(&persist_dir, tenant, &payload.task_id)?
        {
            continue;
        }
        let group = groups
            .entry((
                key.scope_id.clone(),
                key.session_id.clone(),
                key.subject.as_str().to_string(),
            ))
            .or_insert((0, u64::MAX));
        group.0 += 1;
        group.1 = group.1.min(payload.last_turn_ms);
    }
    groups
        .into_iter()
        .map(|((scope_id, session_id, subject), (pending, oldest_ms))| {
            Ok(DerivationStatus {
                scope_id,
                session_id,
                subject: medulla_work_types::participant::ParticipantId::new(subject)
                    .map_err(|e| e.to_string())?,
                pending,
                oldest_pending_age_ms: now_ms.saturating_sub(oldest_ms),
            })
        })
        .collect()
}
