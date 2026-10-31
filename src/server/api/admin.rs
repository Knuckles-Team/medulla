//! Admin and decide handler seams (`AdminApi`, `DecideApi`).

#[cfg(feature = "shacl")]
pub(crate) mod compiled;
#[cfg(feature = "shacl")]
pub mod compose;
#[cfg(feature = "redb")]
pub mod saga;
#[cfg(feature = "shacl")]
pub mod schema_read;
#[cfg(feature = "decide")]
pub mod served_adapter;
#[cfg(feature = "shacl")]
pub mod terms;
pub mod topology_shape;

use std::sync::Arc;

use tokio::sync::RwLock;

use crate::server::state::ServerState;

/// Admin handler behaviour reachable from crates below `medulla-h-admin`.
#[async_trait::async_trait]
pub trait AdminApi: Send + Sync {
    /// Apply one `GraphSchema` operation inside the mutation gateway.
    #[cfg(feature = "shacl")]
    async fn graph_schema_gateway(
        &self,
        state: &Arc<RwLock<ServerState>>,
        tenant_id: &str,
        ctx: &crate::server::mutation::MutationCtx<'_>,
        plan: &crate::server::mutation::MutationPlan,
        method: &crate::protocol::Method,
        op: &medulla_types::graph_schema::GraphSchemaOp,
    ) -> crate::protocol::Response;
}

/// Decide handler behaviour reachable from crates below `medulla-h-decide`.
#[async_trait::async_trait]
pub trait DecideApi: Send + Sync {
    /// The adapter active for `graph` under the verified carrier tenant `tenant`, or `None`
    /// when there is none or its state cannot be read.
    #[cfg(feature = "decide")]
    async fn served_adapter(
        &self,
        state: &Arc<RwLock<ServerState>>,
        tenant: Option<&str>,
        graph: &str,
        core: &crate::graph::GraphCore,
    ) -> Option<Arc<served_adapter::ServedAdapter>>;

    /// The caller's read-only SQL relations (the decision record views) when `query` can see
    /// them.
    #[cfg(feature = "query")]
    async fn read_only_relations(
        &self,
        state: &Arc<RwLock<ServerState>>,
        authority: &crate::server::access::CarrierAuthority,
        query: &str,
    ) -> Option<crate::server::sql_catalog_acl::SharedRelations>;

    /// The caller's visible decision log as a UQL `DECISIONS` source.
    #[cfg(all(feature = "decide", feature = "query"))]
    async fn decision_source(
        &self,
        state: &Arc<RwLock<ServerState>>,
        authority: &crate::server::access::CarrierAuthority,
    ) -> Option<Arc<dyn medulla_plan::exec::DecisionSource>>;
}

/// The installed decide seam of `state`, or `None` before installation.
pub async fn decide_seam(state: &Arc<RwLock<ServerState>>) -> Option<Arc<dyn DecideApi>> {
    state.read().await.seams.decide.get().cloned()
}

/// The installed admin seam of `state`, or `None` before installation.
pub async fn admin_seam(state: &Arc<RwLock<ServerState>>) -> Option<Arc<dyn AdminApi>> {
    state.read().await.seams.admin.get().cloned()
}
