//! Served participant reads (MEDULLA-DECISION-ENGINE-R655.4).
//!
//! `Method::ParticipantRead` is a graph read: the handler reads the projected core under the
//! verified tenant and principal and commits nothing. One `match` over [`ParticipantReadOp`] picks
//! the resolver. Every refusal of an absent participant, a participant of another tenant or one the
//! caller does not act as is the same `PARTICIPANT_NOT_VISIBLE` envelope, so a reader cannot tell
//! which of the three it hit.

use super::participant_surface::{participant_unavailable, readable_participants};
use super::*;
use medulla_decision_types::participant_memory::ParticipantReadOp;
use medulla_protocol_types::result_contract::coordination::{
    ParticipantReadDeriveInput, ParticipantReadGet, ParticipantReadList,
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
            refuse("ACCESS_DENIED", "a participant read needs a verified tenant"),
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
        ParticipantReadOp::Get { participant_id } => readable_participants(ctx.core, tenant, agent_id)
            .into_iter()
            .find(|participant| &participant.participant_id == participant_id)
            .ok_or_else(|| participant_unavailable(participant_id.as_str()))
            .and_then(ResultPayload::of::<ParticipantReadGet>),
        ParticipantReadOp::List => {
            ResultPayload::of::<ParticipantReadList>(readable_participants(ctx.core, tenant, agent_id))
        }
        ParticipantReadOp::DeriveInput { payload_ref } => {
            super::participant_derive::derive_input(ctx.core, tenant, payload_ref)
                .and_then(ResultPayload::of::<ParticipantReadDeriveInput>)
        }
        ParticipantReadOp::Ask { .. } | ParticipantReadOp::Card { .. } | ParticipantReadOp::Status { .. } => {
            Err(refuse("INVALID_ARGUMENT", "this participant read is not served yet"))
        }
    }
}
