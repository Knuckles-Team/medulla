//! Served agent bus roster (MEDULLA-DURABLE-KERNEL-R095.1001, R095.1003, R095.1004).
//!
//! A participant is the node `bus:participant:<session_ref>` with label
//! `BusParticipant` in the request graph. `BusRegister` replaces the node,
//! `BusHeartbeat` upserts `last_seen_ms` in O(1), and `BusRoster` scans the
//! tenant's participants in O(n), ordered by `session_ref`. Writes go through
//! the ordinary gateway commit, so a participant survives restart. Every
//! refusal is decided before the commit and writes nothing. The `bus:read` /
//! `bus:write` scope check (`BUS_SCOPE_REQUIRED`) runs at the request boundary
//! before this module is reached.

use std::sync::atomic::{AtomicU64, Ordering};

use medulla_graph_core::graph::GraphCore;
use medulla_protocol_types::protocol::{Method, Response, ResultPayload};
use medulla_server_mutation::mutation::{MutationCtx, MutationPlan};
use medulla_server_ops::api::ingest::graph::commit_gateway;
use medulla_types::agent_bus::{
    participant_node_id, BusParticipant, BusRoster, BusRosterEntry, BUS_PARTICIPANT_LABEL,
    MAX_PARTICIPANTS_PER_TENANT, MAX_SESSION_REF_BYTES,
};
use medulla_types::contract::ServerErrorCode;

/// Default `bus.participant_ttl_s`.
pub const DEFAULT_PARTICIPANT_TTL_S: u64 = 90;
/// Inclusive bounds of `bus.participant_ttl_s`.
pub const PARTICIPANT_TTL_BOUNDS_S: (u64, u64) = (10, 3600);

static PARTICIPANT_TTL_S: AtomicU64 = AtomicU64::new(DEFAULT_PARTICIPANT_TTL_S);

/// Set `bus.participant_ttl_s` (`--bus-participant-ttl-s`). Refuses a value
/// outside [`PARTICIPANT_TTL_BOUNDS_S`].
pub fn set_participant_ttl_s(ttl_s: u64) -> Result<(), String> {
    let (lo, hi) = PARTICIPANT_TTL_BOUNDS_S;
    if !(lo..=hi).contains(&ttl_s) {
        return Err(format!(
            "{}: bus.participant_ttl_s must be in [{lo}, {hi}], got {ttl_s}",
            ServerErrorCode::InvalidConfiguration.as_str()
        ));
    }
    PARTICIPANT_TTL_S.store(ttl_s, Ordering::Relaxed);
    Ok(())
}

/// The configured `bus.participant_ttl_s`.
pub fn participant_ttl_s() -> u64 {
    PARTICIPANT_TTL_S.load(Ordering::Relaxed)
}

fn refuse(req_id: u64, code: ServerErrorCode, detail: impl std::fmt::Display) -> Response {
    Response::err(req_id, format!("{}: {detail}", code.as_str()))
}

/// The acting principal: the verified principal the request boundary stamped
/// on the method, else the transport caller (an unauthenticated local mode).
fn principal_of(ctx: &MutationCtx<'_>, stamped: &str) -> String {
    if stamped.is_empty() {
        ctx.caller.unwrap_or("anonymous").to_string()
    } else {
        stamped.to_string()
    }
}

/// The stored participant of `session_ref`, if this tenant holds one.
fn lookup(core: &GraphCore, session_ref: &str) -> Option<(BusParticipant, String)> {
    core.get_node_properties(&participant_node_id(session_ref))
        .and_then(|bytes| BusParticipant::from_node_props(&bytes))
}

/// The tenant's participants, `session_ref` ascending. Another tenant's rows
/// are never returned.
fn tenant_participants(core: &GraphCore, tenant: &str) -> Vec<(BusParticipant, String)> {
    let mut rows: Vec<(BusParticipant, String)> = core
        .get_nodes_by_label(BUS_PARTICIPANT_LABEL, usize::MAX)
        .into_iter()
        .filter_map(|(_, bytes)| BusParticipant::from_node_props(&bytes))
        .filter(|(participant, _)| participant.tenant == tenant)
        .collect();
    rows.sort_by(|a, b| a.0.session_ref.cmp(&b.0.session_ref));
    rows
}

/// Liveness rule: live when `now - last_seen_ms` is at most the ttl.
pub fn is_live(now_ms: u64, last_seen_ms: u64, ttl_s: u64) -> bool {
    now_ms.saturating_sub(last_seen_ms) <= ttl_s.saturating_mul(1000)
}

/// `BusRegister` under the verified `tenant` (the request tenant, never the
/// opaque carrier scope, so `BusRoster` finds the row by the same tenant).
pub(crate) async fn register(
    ctx: &MutationCtx<'_>,
    plan: &MutationPlan,
    tenant: &str,
    participant: &BusParticipant,
    principal: &str,
) -> Response {
    let mut participant = participant.clone();
    participant.tenant = tenant.to_string();
    if let Err(detail) = participant.check_bounds() {
        return refuse(ctx.req_id, ServerErrorCode::BusFieldTooLarge, detail);
    }
    let principal = principal_of(ctx, principal);
    match lookup(ctx.core, &participant.session_ref) {
        Some((held, owner)) if held.tenant != participant.tenant || owner != principal => {
            return refuse(
                ctx.req_id,
                ServerErrorCode::BusSessionForeign,
                "session is registered by another principal",
            );
        }
        Some(_) => {}
        None => {
            if tenant_participants(ctx.core, tenant).len() >= MAX_PARTICIPANTS_PER_TENANT {
                return refuse(
                    ctx.req_id,
                    ServerErrorCode::BusRosterFull,
                    format!("a tenant holds at most {MAX_PARTICIPANTS_PER_TENANT} participants"),
                );
            }
        }
    }
    let id = participant_node_id(&participant.session_ref);
    let props = participant.to_node_props(&principal);
    let stamped = Method::BusRegister {
        participant,
        principal,
    };
    commit_gateway(ctx, plan, &stamped, move |core| {
        core.add_node(id, props);
        Ok(ResultPayload::Bool(true))
    })
    .await
}

pub(crate) async fn heartbeat(
    ctx: &MutationCtx<'_>,
    plan: &MutationPlan,
    tenant: &str,
    session_ref: &str,
    now_ms: u64,
    principal: &str,
) -> Response {
    if session_ref.is_empty() || session_ref.len() > MAX_SESSION_REF_BYTES {
        return refuse(
            ctx.req_id,
            ServerErrorCode::BusFieldTooLarge,
            format!("session_ref must be 1..={MAX_SESSION_REF_BYTES} bytes"),
        );
    }
    let principal = principal_of(ctx, principal);
    let Some((held, owner)) = lookup(ctx.core, session_ref) else {
        return Response::ok(ctx.req_id, ResultPayload::Bool(false));
    };
    if held.tenant != tenant || owner != principal {
        return refuse(
            ctx.req_id,
            ServerErrorCode::BusSessionForeign,
            "session is registered by another principal",
        );
    }
    let id = participant_node_id(session_ref);
    let stamped = Method::BusHeartbeat {
        session_ref: session_ref.to_string(),
        now_ms,
        principal,
    };
    commit_gateway(ctx, plan, &stamped, move |core| {
        let Some(props) = core
            .get_node_properties(&id)
            .and_then(|bytes| BusParticipant::heartbeat_props(&bytes, now_ms))
        else {
            return Ok(ResultPayload::Bool(false));
        };
        core.add_node(id, props);
        Ok(ResultPayload::Bool(true))
    })
    .await
}

/// `BusRoster`: a plain read over the (row-filtered) serving core. The request
/// boundary has already bound `tenant` to the verified request tenant.
pub(crate) fn roster(req_id: u64, core: &GraphCore, tenant: &str, now_ms: u64) -> Response {
    let ttl_s = participant_ttl_s();
    let participants = tenant_participants(core, tenant)
        .into_iter()
        .map(|(participant, principal)| BusRosterEntry {
            live: is_live(now_ms, participant.last_seen_ms, ttl_s),
            participant,
            principal,
        })
        .collect();
    match ResultPayload::of::<medulla_protocol_types::result_contract::graph::BusRoster>(
        BusRoster { participants },
    ) {
        Ok(payload) => Response::ok(req_id, payload),
        Err(error) => Response::err(req_id, error),
    }
}
