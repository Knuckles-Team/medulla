//! Query, transaction and wire seams (`QueryApi`, `TxnApi`, `WireApi`).

#[cfg(all(any(test, feature = "test-support"), feature = "query"))]
pub mod edge_test_support;
/// Owner-scoped foreign-source catalog (CONCEPT:MEDULLA-KG.query.query-federation, EH-373): the one
/// place a caller's verified owner (tenant+principal) selects which sources a plan may resolve.
#[cfg(feature = "federation")]
pub mod foreign_catalog;
/// MEDULLA-FEDERATED-QUERY-R722.3 — bounded host-driven foreign-read executions.
#[cfg(feature = "federation")]
pub mod foreign_read;
/// Explicit RBAC-grant sharing of an owner-scoped foreign source (EH-378).
#[cfg(feature = "federation")]
pub mod foreign_share;
pub mod query_support;
pub mod txn_intent;
pub mod txn_support;

use std::sync::Arc;

use tokio::sync::RwLock;

use medulla_server_api::state::ServerState;

/// Query handler behaviour reachable from crates below `medulla-h-query`.
#[async_trait::async_trait]
pub trait QueryApi: Send + Sync {
    /// The semantic owner of an EXISTING binding, for operator reads: unlike opening a
    /// binding it never creates an owner file for a binding that was never admitted.
    #[cfg(feature = "ann-redb")]
    fn existing_semantic_service(
        &self,
        persist_dir: &std::path::Path,
        tenant: &str,
        binding_id: &str,
    ) -> Result<Arc<medulla_ann_codes::semantic_index_service::SemanticIndexService>, String>;

    /// Run `plan` over a staged (transaction-overlaid) view with the core indexes, committed
    /// and staged embeddings, and the caller's governed foreign and time-series legs bound,
    /// projecting the rows as `[id, score|nil]`.
    #[cfg(feature = "query")]
    fn run_unified_staged_rows(
        &self,
        plan: medulla_plan::Plan,
        view: &medulla_graph_core::graph::GraphView,
        core: &Arc<medulla_graph_core::graph::GraphCore>,
        staged: &[(String, Vec<f32>)],
        #[cfg(feature = "federation")] foreign: Option<
            &medulla_plan::federation::ForeignSourceRegistry,
        >,
        #[cfg(feature = "tsdb")] tsdb_ctx: query_support::TsdbLegBind<'_>,
    ) -> Result<Vec<(String, Option<f32>)>, String>;

    /// Route one gateway-routed data-mining method (`Mine*`) through the mining handler's
    /// commit gateway; `None` hands a method it does not own back to the caller.
    #[cfg(feature = "mining")]
    async fn mining_gateway_try_handle(
        &self,
        ctx: &medulla_server_mutation::mutation::MutationCtx<'_>,
        plan: &medulla_server_mutation::mutation::MutationPlan,
        method: &medulla_protocol_types::protocol::Method,
        graph_name: &str,
        read_authority: Option<&medulla_server_api::carrier::GraphReadAuthority>,
        #[cfg(all(feature = "query", feature = "tsdb"))] tsdb_store: Option<
            &Arc<medulla_tsdb::store::SeriesStore>,
        >,
    ) -> Option<medulla_protocol_types::protocol::Response>;
}

/// Transaction handler behaviour reachable from crates below `medulla-h-txn`.
#[async_trait::async_trait]
pub trait TxnApi: Send + Sync {
    /// Route one transaction-family method through the transaction handler.
    async fn try_handle(
        &self,
        state: &Arc<RwLock<ServerState>>,
        req_id: u64,
        caller: &str,
        verified_context: &medulla_server_api::authority_context::VerifiedRequestContext,
        method: medulla_protocol_types::protocol::Method,
    ) -> Result<medulla_protocol_types::protocol::Response, medulla_protocol_types::protocol::Method>;

    /// Route one `Ts*` method through the time-series handler.
    #[cfg(feature = "tsdb")]
    async fn timeseries_try_handle(
        &self,
        state: &Arc<RwLock<ServerState>>,
        req_id: u64,
        authority: &medulla_server_api::carrier::CarrierAuthority,
        graph: &str,
        placement_epoch: u64,
        fencing_token: Option<u64>,
        method: medulla_protocol_types::protocol::Method,
    ) -> Result<medulla_protocol_types::protocol::Response, medulla_protocol_types::protocol::Method>;

    /// Land a staged cross-modal transaction durably with an optional replay nonce.
    async fn commit_cross_modal_txn_with_nonce(
        &self,
        state: &Arc<RwLock<ServerState>>,
        request_id: u64,
        caller: Option<&str>,
        coordinator_id: &str,
        txn: medulla_server_api::txn::GraphTxnState,
        attempt_nonce: Option<medulla_types::contract::Nonce>,
    ) -> Result<bool, String>;

    /// Land a GraphQL-staged cross-modal transaction durably.
    #[cfg(feature = "graphql")]
    async fn commit_graphql_cross_modal(
        &self,
        ctx: txn_support::GraphQlCrossModalCommit<'_>,
    ) -> Result<bool, String>;

    /// Admit a GraphQL begin/stage/read/rollback operation to the transaction lifecycle saga.
    #[cfg(feature = "graphql")]
    async fn begin_graphql_lifecycle(
        &self,
        state: &Arc<RwLock<ServerState>>,
        req_id: u64,
        caller: &str,
        authority: &medulla_server_api::carrier::CarrierAuthority,
        method: &medulla_protocol_types::protocol::Method,
    ) -> Result<txn_support::GraphQlLifecycleAdmission, String>;

    /// Terminalize an admitted GraphQL lifecycle operation with its result.
    #[cfg(feature = "graphql")]
    fn finish_graphql_lifecycle(
        &self,
        token: txn_support::LifecycleToken,
        result: medulla_protocol_types::protocol::ResultPayload,
    ) -> Result<medulla_protocol_types::protocol::ResultPayload, String>;

    /// Reload every persisted materialized view into the in-RAM index; returns the count.
    #[cfg(any(feature = "compute-dist", feature = "matview"))]
    async fn reload_matviews(&self, state: &Arc<RwLock<ServerState>>) -> Result<usize, String>;
}

/// Wire protocol behaviour reachable from crates below `medulla-wire`.
pub trait WireApi: Send + Sync {
    /// Authorize (and RLS-rewrite in place) every op of a buffered table transaction before it
    /// may be committed; returns the schemas of the tables the batch creates. The error is
    /// the caller-facing message.
    #[cfg(feature = "query")]
    fn authorize_table_txn(
        &self,
        source: &medulla_server_auth::sql_catalog_acl::SqlSourceAuthorityWrite<'_, '_>,
        table_store: &medulla_table_store::tables::TableStore,
        txn: &mut medulla_table_store::tables::TableTxn,
        committed_replay: bool,
    ) -> Result<Vec<medulla_tables::schema::TableSchema>, String>;

    /// The committed SQL receipt that authorizes owner repair for a crash replay.
    #[cfg(feature = "query")]
    fn committed_sql_replay_receipt(
        &self,
        store: &medulla_table_store::tables::TableStore,
        authority: &medulla_server_api::carrier::CarrierAuthority,
        batch: &medulla_protocol_types::mutation_batch::MutationBatch,
    ) -> Result<Option<medulla_protocol_types::mutation_batch::MutationBatchRecord>, String>;
}

/// The installed query seam of `state`, or `None` before installation.
pub async fn query_seam(state: &Arc<RwLock<ServerState>>) -> Option<Arc<dyn QueryApi>> {
    state
        .read()
        .await
        .seams
        .query
        .get()
        .and_then(|h| h.downcast_ref::<Arc<dyn QueryApi>>())
        .cloned()
}

/// Install the query seam; `false` when it was already installed.
pub fn install_query_seam(reg: &medulla_server_api::api::registry::SeamRegistry, api: Arc<dyn QueryApi>) -> bool {
    reg.query.set(Arc::new(api) as medulla_server_api::state::ErasedHandle).is_ok()
}

/// The installed txn seam of `state`, or `None` before installation.
pub async fn txn_seam(state: &Arc<RwLock<ServerState>>) -> Option<Arc<dyn TxnApi>> {
    state
        .read()
        .await
        .seams
        .txn
        .get()
        .and_then(|h| h.downcast_ref::<Arc<dyn TxnApi>>())
        .cloned()
}

/// Install the txn seam; `false` when it was already installed.
pub fn install_txn_seam(reg: &medulla_server_api::api::registry::SeamRegistry, api: Arc<dyn TxnApi>) -> bool {
    reg.txn.set(Arc::new(api) as medulla_server_api::state::ErasedHandle).is_ok()
}

/// The installed wire seam of `state`, or `None` before installation.
pub async fn wire_seam(state: &Arc<RwLock<ServerState>>) -> Option<Arc<dyn WireApi>> {
    state
        .read()
        .await
        .seams
        .wire
        .get()
        .and_then(|h| h.downcast_ref::<Arc<dyn WireApi>>())
        .cloned()
}

/// Install the wire seam; `false` when it was already installed.
pub fn install_wire_seam(reg: &medulla_server_api::api::registry::SeamRegistry, api: Arc<dyn WireApi>) -> bool {
    reg.wire.set(Arc::new(api) as medulla_server_api::state::ErasedHandle).is_ok()
}

/// Typed access to the foreign-source catalog and read sessions `ServerState`
/// holds type-erased (MEDULLA-CONTRACT-R056.8). An untouched slot lazily
/// becomes an empty catalog or session table.
#[cfg(feature = "federation")]
pub trait ForeignStateExt {
    /// The registered foreign-source catalog.
    fn foreign_sources(&self) -> Arc<foreign_catalog::ForeignSourceCatalog>;
    /// The host-driven foreign-read page buffers.
    fn foreign_reads(&self) -> Arc<foreign_read::ForeignReadSessions>;
}

#[cfg(feature = "federation")]
impl ForeignStateExt for ServerState {
    fn foreign_sources(&self) -> Arc<foreign_catalog::ForeignSourceCatalog> {
        Arc::clone(self.foreign_sources.get_or_init(|| {
            Arc::new(foreign_catalog::ForeignSourceCatalog::default())
                as medulla_server_api::state::ErasedHandle
        }))
        .downcast::<foreign_catalog::ForeignSourceCatalog>()
        .expect("foreign_sources slot holds ForeignSourceCatalog")
    }

    fn foreign_reads(&self) -> Arc<foreign_read::ForeignReadSessions> {
        Arc::clone(self.foreign_reads.get_or_init(|| {
            Arc::new(foreign_read::ForeignReadSessions::default()) as medulla_server_api::state::ErasedHandle
        }))
        .downcast::<foreign_read::ForeignReadSessions>()
        .expect("foreign_reads slot holds ForeignReadSessions")
    }
}

#[cfg(all(test, feature = "federation"))]
mod foreign_state_tests {
    use super::ForeignStateExt;
    use medulla_server_api::state::ServerState;
    use medulla_isolation::isolation::IsolationLayer;

    #[test]
    fn untouched_state_has_an_empty_catalog() {
        let state = ServerState::new("secret", IsolationLayer::new());
        assert!(state.foreign_sources().is_empty());
    }
}
