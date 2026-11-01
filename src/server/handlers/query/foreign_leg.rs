//! Served foreign-source leg resolution over the server's catalog (federation).

use super::*;
use medulla_server_api::api::query_txn::foreign_catalog::OwnedForeignRegistry;
#[cfg(feature = "redb")]
use medulla_persistence::agent_library::AgentLibraryStateExt;

/// [`ForeignSourceCatalog::resolve_for_plan`] against the server's catalog for a served
/// plan, with the caller's verified carrier taken from its read authority. `Err` is the
/// caller-facing refusal text.
pub(crate) async fn served_foreign_leg(
    state: &Arc<RwLock<ServerState>>,
    plan: &medulla_plan::Plan,
    read_authority: Option<&GraphReadAuthority>,
    execution: Option<&medulla_types::foreign_read::ForeignReadExecutionRef>,
    executions: &[medulla_types::foreign_read::ForeignReadExecutionRef],
) -> Result<Option<OwnedForeignRegistry>, String> {
    let s = state.read().await;
    let caller = read_authority.and_then(GraphReadAuthority::carrier);
    if execution.is_some() && !executions.is_empty() {
        return Err("INVALID_ARGUMENT: conflicting foreign execution forms".into());
    }
    if (execution.is_some() || !executions.is_empty())
        && !medulla_server_api::api::query_txn::query_support::plan_needs_foreign(&plan.ops)
    {
        return Err(
            "FOREIGN_READ_UNUSED: execution is attached to a plan without a foreign source".into(),
        );
    }
    if !executions.is_empty() {
        let caller = caller.ok_or_else(|| {
            "ACCESS_DENIED: foreign read executions require a verified tenant carrier".to_string()
        })?;
        let store_arc = s.agent_library();
        let store = store_arc.as_deref().ok_or_else(|| {
            "SOURCE_APPROVAL_STORE_UNAVAILABLE: foreign read requires the Agent Library".to_string()
        })?;
        let plan_digest =
            medulla_server_api::api::query_txn::query_support::canonical_plan_digest(plan)?;
        return s
            .foreign_reads
            .bind_many_for_query(
                caller,
                executions,
                plan,
                plan_digest,
                medulla_server_api::api::query_txn::foreign_read::ForeignBindingContext {
                    catalog: &s.foreign_sources,
                    store,
                    isolation: &s.isolation,
                },
            )
            .map(Some);
    }
    if let Some(execution) = execution {
        let caller = caller.ok_or_else(|| {
            "ACCESS_DENIED: a foreign read execution requires a verified tenant carrier".to_string()
        })?;
        let store_arc = s.agent_library();
        let store = store_arc.as_deref().ok_or_else(|| {
            "SOURCE_APPROVAL_STORE_UNAVAILABLE: foreign read requires the Agent Library".to_string()
        })?;
        let plan_digest =
            medulla_server_api::api::query_txn::query_support::canonical_plan_digest(plan)?;
        return s
            .foreign_reads
            .bind_for_query(
                caller,
                *execution,
                plan_digest,
                &s.foreign_sources,
                store,
                &s.isolation,
            )
            .map(Some);
    }
    s.foreign_sources
        .resolve_for_plan(&plan.ops, caller, &s.isolation)
}
