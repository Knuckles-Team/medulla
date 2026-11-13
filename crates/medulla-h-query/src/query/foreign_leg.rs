//! Served foreign-source leg resolution over the server's catalog (federation).

use super::*;
#[cfg(feature = "redb")]
use medulla_persistence_records::agent_library::AgentLibraryStateExt;
use medulla_server_ops::api::query_txn::foreign_catalog::OwnedForeignRegistry;
use medulla_server_ops::api::query_txn::ForeignStateExt;

/// [`ForeignSourceCatalog::resolve_for_plan`] against the server's catalog for a served
/// plan, with the caller's verified carrier taken from its read authority. `Err` is the
/// caller-facing refusal text.
pub(crate) async fn served_foreign_leg(
    state: &Arc<RwLock<ServerState>>,
    plan: &medulla_exec::algebra::Plan,
    read_authority: Option<&GraphReadAuthority>,
    execution: Option<&medulla_catalog_types::foreign_read::ForeignReadExecutionRef>,
    executions: &[medulla_catalog_types::foreign_read::ForeignReadExecutionRef],
) -> Result<ForeignLeg, String> {
    let s = state.read().await;
    let caller = read_authority.and_then(GraphReadAuthority::carrier);
    if execution.is_some() && !executions.is_empty() {
        return Err("INVALID_ARGUMENT: conflicting foreign execution forms".into());
    }
    if (execution.is_some() || !executions.is_empty())
        && !medulla_server_ops::api::query_txn::query_support::plan_needs_foreign(&plan.ops)
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
            medulla_server_ops::api::query_txn::query_support::canonical_plan_digest(plan)?;
        let mut registry = s.foreign_reads().bind_many_for_query(
            caller,
            executions,
            plan,
            plan_digest,
            medulla_server_ops::api::query_txn::foreign_read::ForeignBindingContext {
                catalog: &s.foreign_sources(),
                store,
                isolation: &s.isolation,
            },
        )?;
        // MEDULLA-CQ-CONN-R090: a source the caller cannot read adds no rows and is
        // named in the coverage block; the readable sources still answer.
        let names =
            medulla_server_ops::api::query_txn::query_support::plan_foreign_names(&plan.ops);
        let coverage = registry
            .exclude_unreadable(&names)
            .into_iter()
            .map(|(source, used)| medulla_types::wire::UqlSourceCoverage {
                source,
                used,
                reason: (!used).then(unreadable_reason),
            })
            .collect();
        return Ok(ForeignLeg {
            registry: Some(registry),
            coverage,
        });
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
            medulla_server_ops::api::query_txn::query_support::canonical_plan_digest(plan)?;
        return s
            .foreign_reads()
            .bind_for_query(
                caller,
                *execution,
                plan_digest,
                &s.foreign_sources(),
                store,
                &s.isolation,
            )
            .map(|registry| ForeignLeg {
                registry: Some(registry),
                coverage: Vec::new(),
            });
    }
    s.foreign_sources()
        .resolve_for_plan(&plan.ops, caller, &s.isolation)
        .map(|registry| ForeignLeg {
            registry,
            coverage: Vec::new(),
        })
}

/// What the foreign leg of one served plan resolved to: the caller's registry (when the
/// plan names a source) and the per-source coverage of a host-driven read.
#[derive(Default)]
pub(crate) struct ForeignLeg {
    pub(crate) registry: Option<OwnedForeignRegistry>,
    pub(crate) coverage: Vec<medulla_types::wire::UqlSourceCoverage>,
}

/// The wire reason of an unreadable source: the source-authority reason `Unconnected`
/// (absent from the caller's owner-scoped catalog), rendered by its own serde form so
/// the coverage block has no second list of reasons.
fn unreadable_reason() -> String {
    match serde_json::to_value(medulla_identity_types::authority_audit::NotUsedReason::Unconnected)
    {
        Ok(serde_json::Value::String(reason)) => reason,
        _ => "unconnected".to_string(),
    }
}
