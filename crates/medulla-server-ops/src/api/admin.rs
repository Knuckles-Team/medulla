//! Admin and decide handler seams (`AdminApi`, `DecideApi`).

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

use medulla_server_api::state::ServerState;

/// Admin handler behaviour reachable from crates below `medulla-h-admin`.
#[async_trait::async_trait]
pub trait AdminApi: Send + Sync {
    /// Apply one `GraphSchema` operation inside the mutation gateway.
    #[cfg(feature = "shacl")]
    async fn graph_schema_gateway(
        &self,
        state: &Arc<RwLock<ServerState>>,
        tenant_id: &str,
        ctx: &medulla_server_api::mutation::MutationCtx<'_>,
        plan: &medulla_server_api::mutation::MutationPlan,
        method: &medulla_protocol_types::protocol::Method,
        op: &medulla_catalog_types::graph_schema::GraphSchemaOp,
    ) -> medulla_protocol_types::protocol::Response;
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
        core: &medulla_graph_core::graph::GraphCore,
    ) -> Option<Arc<served_adapter::ServedAdapter>>;

    /// The caller's read-only SQL relations (the decision record views) when `query` can see
    /// them.
    #[cfg(feature = "query")]
    async fn read_only_relations(
        &self,
        state: &Arc<RwLock<ServerState>>,
        authority: &medulla_server_api::carrier::CarrierAuthority,
        query: &str,
    ) -> Option<medulla_server_api::sql_catalog_acl::SharedRelations>;

    /// The caller's visible decision log as a UQL `DECISIONS` source.
    #[cfg(all(feature = "decide", feature = "query"))]
    async fn decision_source(
        &self,
        state: &Arc<RwLock<ServerState>>,
        authority: &medulla_server_api::carrier::CarrierAuthority,
    ) -> Option<Arc<dyn medulla_plan::exec::DecisionSource>>;
}

/// The installed decide seam of `state`, or `None` before installation.
pub async fn decide_seam(state: &Arc<RwLock<ServerState>>) -> Option<Arc<dyn DecideApi>> {
    state
        .read()
        .await
        .seams
        .decide
        .get()
        .and_then(|h| h.downcast_ref::<Arc<dyn DecideApi>>())
        .cloned()
}

/// Install the decide seam; `false` when it was already installed.
pub fn install_decide_seam(reg: &medulla_server_api::api::registry::SeamRegistry, api: Arc<dyn DecideApi>) -> bool {
    reg.decide.set(Arc::new(api) as medulla_server_api::state::ErasedHandle).is_ok()
}

/// The installed admin seam of `state`, or `None` before installation.
pub async fn admin_seam(state: &Arc<RwLock<ServerState>>) -> Option<Arc<dyn AdminApi>> {
    state
        .read()
        .await
        .seams
        .admin
        .get()
        .and_then(|h| h.downcast_ref::<Arc<dyn AdminApi>>())
        .cloned()
}

/// Install the admin seam; `false` when it was already installed.
pub fn install_admin_seam(reg: &medulla_server_api::api::registry::SeamRegistry, api: Arc<dyn AdminApi>) -> bool {
    reg.admin.set(Arc::new(api) as medulla_server_api::state::ErasedHandle).is_ok()
}
