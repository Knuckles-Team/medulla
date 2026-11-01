//! M3 catalog-driven resharding ADMIN RPC (CONCEPT:MEDULLA-KG.backend.m3-admin-dispatch).
//!
//! The wire surface that DRIVES the M3 ops the engine already has the building blocks for:
//! online single-node resharding (CONCEPT:MEDULLA-KG.backend.catalog-shard-resolve `RedbBackend::reshard_graph`), the durable
//! tenant catalog (CONCEPT:MEDULLA-KG.sharding.empty-catalog-routing `RedbBackend::catalog`), the rebalancing planner
//! (CONCEPT:MEDULLA-KG.sharding.even-load-rebalance `rebalance::plan_rebalance`), and its execution (CONCEPT:MEDULLA-KG.backend.r3-plan-execution
//! `RedbBackend::rebalance_execute`). These CALL the existing persistence APIs — they do not
//! reimplement them.
//!
//! All are durable-redb-only. The module is always declared so the dispatch routing chain is
//! identical across builds; in a build WITHOUT `redb` the catalog/reshard/planner don't
//! exist, so the handler returns a clean "not available in this build" error.

#[cfg(not(feature = "redb"))]
use medulla_protocol_types::protocol::Method;
use medulla_protocol_types::protocol::Response;
#[cfg(feature = "redb")]
use medulla_persistence::agent_library::AgentLibraryStateExt;
#[cfg(not(feature = "redb"))]
use medulla_server_api::state::ServerState;
#[cfg(not(feature = "redb"))]
use medulla_types::contract::Nonce;
#[cfg(not(feature = "redb"))]
use std::sync::Arc;
#[cfg(not(feature = "redb"))]
use tokio::sync::RwLock;

#[cfg(feature = "redb")]
mod agent;
#[cfg(feature = "redb")]
mod backup;
#[cfg(feature = "redb")]
mod cluster;
#[cfg(feature = "redb")]
mod component_content;
// Declared in every build -- the dispatch arm that reaches it carries no cfg;
// each op refuses by name in a build without its durable substrates.
// `pub(crate)` so the projection worker shares the admin re-projection path.
pub(crate) mod connector_pack;
#[cfg(feature = "redb")]
pub(crate) mod saga;

#[cfg(all(any(test, feature = "test-support"), feature = "redb"))]
pub use agent::bind_agent_library_draft;
#[cfg(feature = "redb")]
pub use agent::{
    handle_agent_component, handle_agent_graph, handle_agent_library, handle_agent_template,
};
#[cfg(all(any(test, feature = "test-support"), feature = "redb"))]
pub use backup::backup_bundle_name;
#[cfg(feature = "redb")]
pub use cluster::try_handle;
pub use connector_pack::handle_connector_pack;

/// Non-redb build: every admin method returns a clean "not available" error.
#[cfg(not(feature = "redb"))]
pub async fn try_handle(
    _state: &Arc<RwLock<ServerState>>,
    req_id: u64,
    _caller: Option<&str>,
    _attempt_nonce: Option<Nonce>,
    method: Method,
) -> Result<Response, Method> {
    match method {
        Method::Reshard { .. }
        | Method::CatalogAssign { .. }
        | Method::CatalogReassign { .. }
        | Method::CatalogRemove { .. }
        | Method::CatalogList
        | Method::RebalancePlan { .. }
        | Method::RebalanceExecute { .. }
        | Method::Backup { .. }
        | Method::Restore { .. } => Ok(Response::err(
            req_id,
            "ENGINE_UNAVAILABLE: M3 resharding admin is not available in this build (requires the `redb` feature)",
        )),
        other => Err(other),
    }
}

#[cfg(all(test, feature = "redb"))]
mod security_tests {
    use super::backup_bundle_name;

    #[test]
    fn backup_bundle_names_are_logical_not_paths() {
        for valid in ["scheduled-001", "snapshot_2", "release.3"] {
            assert_eq!(backup_bundle_name(valid).unwrap(), valid);
        }
        for invalid in [
            "",
            ".hidden",
            "../snapshot",
            "nested/snapshot",
            "C:\\snapshot",
            "snapshot\n",
        ] {
            assert!(backup_bundle_name(invalid).is_err(), "accepted {invalid:?}");
        }
    }
}

/// The admin handler's implementation of [`AdminApi`](medulla_server::api::admin::AdminApi).
pub struct AdminSeam;

#[async_trait::async_trait]
impl medulla_server_api::api::admin::AdminApi for AdminSeam {
    #[cfg(feature = "shacl")]
    async fn graph_schema_gateway(
        &self,
        state: &std::sync::Arc<tokio::sync::RwLock<medulla_server_api::state::ServerState>>,
        tenant_id: &str,
        ctx: &medulla_server_api::mutation::MutationCtx<'_>,
        plan: &medulla_server_api::mutation::MutationPlan,
        method: &medulla_protocol_types::protocol::Method,
        op: &medulla_catalog_types::graph_schema::GraphSchemaOp,
    ) -> medulla_protocol_types::protocol::Response {
        crate::graph_schema::handle_gateway(state, tenant_id, ctx, plan, method, op).await
    }
}
