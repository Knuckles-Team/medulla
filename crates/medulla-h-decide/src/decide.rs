//! The Decide layer's served surface (RF-ADR-010).
//!
//! Four methods, four files: assembly, its commit, the statistical executor
//! and the two admin jobs. Each entry point's signature is frozen by the
//! contract wave; the package that lands a handler replaces only its body.

/// The server state every Decide entry point is handed.
pub type SharedState = std::sync::Arc<tokio::sync::RwLock<medulla_server_api::state::ServerState>>;

pub(crate) mod assemble;
pub(crate) mod commit;
pub(crate) mod coverage;
#[cfg(all(test, feature = "decide"))]
mod coverage_tests;
pub(crate) mod guardrail;
pub(crate) mod jobs;
pub(crate) mod log;
#[cfg(feature = "decide")]
pub(crate) mod shape;
pub(crate) mod statistical;

// The statistical package (EH-056 ... EH-074): candidate sources, the
// decision executor, the NL template binding and the two admin jobs.
#[cfg(feature = "decide")]
mod candidates;
#[cfg(feature = "decide")]
mod stat_adapter;
#[cfg(feature = "decide")]
mod stat_belief;
#[cfg(feature = "decide")]
mod stat_classes;
#[cfg(feature = "decide")]
#[allow(missing_docs)]
pub mod stat_decide;
#[cfg(feature = "decide")]
mod stat_evaluator;
#[cfg(feature = "decide")]
mod stat_executor;
#[cfg(feature = "decide")]
mod stat_generation;
// MEDULLA-DECISION-ENGINE-R803: the contrastive harness composition decision.
#[cfg(feature = "decide")]
mod stat_harness_contrast;
#[cfg(feature = "decide")]
mod stat_jobs;
#[cfg(feature = "decide")]
mod stat_learning;
#[cfg(feature = "decide")]
mod stat_log;
#[cfg(feature = "decide")]
mod stat_nl;
#[cfg(feature = "decide")]
mod stat_pointer;
#[cfg(feature = "decide")]
mod stat_replay;
#[cfg(feature = "decide")]
mod stat_resolve;
#[cfg(feature = "decide")]
mod stat_retention;
#[cfg(feature = "decide")]
mod stat_slate;
// EH-523: the per-component split of a slate's outcome on the outcome aggregate.
#[cfg(feature = "decide")]
mod stat_slate_attribution;
#[cfg(feature = "decide")]
mod stat_support;
// MEDULLA-DECISION-ENGINE-R045: the smallest covering tool subset within a
// context-token budget.
#[cfg(feature = "decide")]
mod stat_task_plan;
#[cfg(feature = "decide")]
mod stat_tool_subset;
#[cfg(feature = "decide")]
mod stat_vectors;
// EH-528: walk-forward replay evaluation (its validation kernels are finance's).
#[cfg(all(feature = "decide", feature = "finance"))]
mod stat_walk_forward;
// EH-394/395: retrieval outcomes on the decision log.
#[cfg(feature = "decide")]
mod stat_retrieval;
// EH-394..398: the retrieval-learning relations of the decision views.
#[cfg(all(feature = "decide", feature = "query"))]
mod stat_retrieval_views;
// EH-525: the learned reputation relation of the decision views.
#[cfg(all(feature = "decide", feature = "query"))]
mod stat_reputation;
// EH-396: the query adapter on the served read paths.
#[cfg(feature = "decide")]
pub(crate) mod served_adapter;
#[cfg(all(any(test, feature = "test-support"), feature = "redb"))]
#[allow(missing_docs)]
pub mod stat_test_support;
#[cfg(all(test, feature = "decide"))]
mod stat_tests;
// EH-066 — the decision record views (SQL catalog relations + the UQL `DECISIONS` source).
#[cfg(all(feature = "decide", feature = "query"))]
mod stat_view;
#[cfg(feature = "decide")]
mod telemetry;

// Swarm topology through the assembly decision (SWARM-TOPOLOGY-DECIDE-DESIGN):
// the schema entailments and capacity headroom a topology question reads, and
// the publish-time topology shape check.
#[cfg(feature = "decide")]
mod capacity_premise;
#[cfg(feature = "decide")]
mod topology_read;
#[cfg(feature = "decide")]
mod topology_schema;

pub use assemble::handle_agent_assemble;
pub use commit::handle_decision_commit;
pub use coverage::handle_capability_coverage;
pub use guardrail::handle_guardrail_entailment;
pub use jobs::{handle_decision_eval, handle_decision_fit};
pub use log::handle_decision_log;
pub use statistical::handle_decide;

/// The caller's read-only SQL relations — the decision record views (EH-066) — when
/// `query` can see them; `None` otherwise, or in a build without the Decide layer.
#[cfg(feature = "query")]
async fn read_only_relations(
    state: &SharedState,
    authority: &medulla_server_api::carrier::CarrierAuthority,
    query: &str,
) -> Option<medulla_server_api::sql_catalog_acl::SharedRelations> {
    #[cfg(feature = "decide")]
    {
        if !medulla_server_api::sql_catalog_acl::wants_read_only_relations(query) {
            return None;
        }
        let views = stat_view::DecisionViews::served(state, authority).await?;
        Some(views as medulla_server_api::sql_catalog_acl::SharedRelations)
    }
    #[cfg(not(feature = "decide"))]
    {
        let _ = (state, authority, query);
        None
    }
}

/// The caller's visible decision log as a UQL `DECISIONS` source (EH-066).
#[cfg(all(feature = "decide", feature = "query"))]
async fn decision_source(
    state: &SharedState,
    authority: &medulla_server_api::carrier::CarrierAuthority,
) -> Option<std::sync::Arc<dyn medulla_plan::exec::DecisionSource>> {
    let views = stat_view::DecisionViews::served(state, authority).await?;
    Some(views as std::sync::Arc<dyn medulla_plan::exec::DecisionSource>)
}

/// The decide handler's implementation of [`DecideApi`](medulla_server::api::admin::DecideApi).
pub struct DecideSeam;

#[async_trait::async_trait]
impl medulla_server_api::api::admin::DecideApi for DecideSeam {
    #[cfg(feature = "decide")]
    async fn served_adapter(
        &self,
        state: &SharedState,
        tenant: Option<&str>,
        graph: &str,
        core: &medulla_graph_core::graph::GraphCore,
    ) -> Option<std::sync::Arc<medulla_server_api::api::admin::served_adapter::ServedAdapter>> {
        served_adapter::served_for(state, tenant, graph, core).await
    }

    #[cfg(feature = "query")]
    async fn read_only_relations(
        &self,
        state: &SharedState,
        authority: &medulla_server_api::carrier::CarrierAuthority,
        query: &str,
    ) -> Option<medulla_server_api::sql_catalog_acl::SharedRelations> {
        read_only_relations(state, authority, query).await
    }

    #[cfg(all(feature = "decide", feature = "query"))]
    async fn decision_source(
        &self,
        state: &SharedState,
        authority: &medulla_server_api::carrier::CarrierAuthority,
    ) -> Option<std::sync::Arc<dyn medulla_plan::exec::DecisionSource>> {
        decision_source(state, authority).await
    }
}
