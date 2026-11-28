//! `GraphOwnershipPlan`: the read-only ownership plan of one graph (MEDULLA-IDENTITY-R700.3).
//!
//! The handler builds the planner's signals from the durable write ledger and creator of
//! MEDULLA-IDENTITY-R700.7, reads the knob `identity.ownership_writer_share`, and answers the
//! deterministic plan with the digests an apply must echo. It changes nothing.

use std::sync::Arc;

use tokio::sync::RwLock;

use medulla_isolation::isolation::ownership::{plan, OwnershipWriterShare};
use medulla_persistence::AsRedb;
use medulla_protocol_types::protocol::{Response, ResultPayload};
use medulla_protocol_types::result_contract::security::GraphOwnershipPlan;
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_server_api::state::ServerState;
use medulla_store::redb_store::write_ledger::day_of;
use {
    medulla_identity_core_types::identity::plan_digest,
    medulla_identity_core_types::identity::signals_digest,
    medulla_identity_core_types::identity::AppliedSummary,
    medulla_identity_core_types::identity::GraphOwnershipPlanReply,
    medulla_identity_core_types::identity::GraphOwnershipSignals,
    medulla_identity_core_types::identity::OWNERSHIP_SCHEMA_VERSION,
};

/// The window of `writes_30d`: today and the 29 days before it.
const WINDOW_DAYS: u32 = 30;

/// The part of the graph name after a leading `tenant:` or `usr:`.
fn name_hint(graph: &str) -> Option<String> {
    graph
        .strip_prefix("tenant:")
        .or_else(|| graph.strip_prefix("usr:"))
        .filter(|rest| !rest.is_empty())
        .map(str::to_string)
}

/// Serve one `GraphOwnershipPlan`.
pub async fn handle_plan(
    state: &Arc<RwLock<ServerState>>,
    req_id: u64,
    graph: &str,
    _verified_context: &VerifiedRequestContext,
) -> Response {
    let (backend, resident, applied) = {
        let guard = state.read().await;
        let applied = guard
            .isolation
            .rbac()
            .identity_store()
            .latest_ownership_record(graph)
            .map(|record| AppliedSummary {
                plan_digest: record.plan_digest.clone(),
                actor: record.actor.clone(),
                applied_at_ms: record.applied_at_ms,
                rolled_back: record.rolled_back,
            });
        (
            guard.persistence.clone(),
            guard.registry.get(graph).is_some(),
            applied,
        )
    };
    if !resident {
        return Response::err(
            req_id,
            format!("INVALID_ARGUMENT: graph '{graph}' does not exist"),
        );
    }
    let Some(redb) = backend.as_ref().and_then(|backend| backend.as_redb()) else {
        return Response::err(
            req_id,
            "INVALID_ARGUMENT: ownership signals require the durable store",
        );
    };
    let fname = medulla_kernel::persist::sanitize(graph);
    let today = day_of(medulla_kernel::kernel::dispatch_support::authoritative_now_ms());
    let since_day = today.saturating_sub(WINDOW_DAYS - 1);
    let (writers, creator) = match (
        redb.write_days(&fname, since_day).await,
        redb.graph_creator(&fname).await,
    ) {
        (Ok(writers), Ok(creator)) => (writers, creator),
        (Err(error), _) | (_, Err(error)) => {
            return Response::err(
                req_id,
                format!("INTERNAL: ownership signals could not be read: {error}"),
            )
        }
    };
    let signals = GraphOwnershipSignals {
        schema_version: OWNERSHIP_SCHEMA_VERSION,
        graph: graph.to_string(),
        creator,
        writers,
        name_hint: name_hint(graph),
    };
    if let Err(error) = signals.validate() {
        return Response::err(
            req_id,
            format!("INTERNAL: ownership signals invalid: {error:?}"),
        );
    }
    let share = match OwnershipWriterShare::new(
        medulla_server_api::knob_config::knobs().identity_ownership_writer_share,
    ) {
        Ok(share) => share,
        Err(error) => return Response::err(req_id, format!("INTERNAL: {error}")),
    };
    let entries = plan(&signals, share);
    let reply = GraphOwnershipPlanReply {
        schema_version: OWNERSHIP_SCHEMA_VERSION,
        graph: graph.to_string(),
        signals_digest: signals_digest(&signals),
        plan_digest: plan_digest(&entries),
        signals,
        plan: entries,
        applied,
    };
    Response::ok(req_id, ResultPayload::of::<GraphOwnershipPlan>(reply))
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-IDENTITY-R700.3
    #[test]
    fn name_hint_strips_the_tenant_or_user_prefix() {
        assert_eq!(name_hint("tenant:acme"), Some("acme".to_string()));
        assert_eq!(name_hint("usr:alice"), Some("alice".to_string()));
        assert_eq!(name_hint("plain"), None);
        assert_eq!(name_hint("tenant:"), None);
    }
}
