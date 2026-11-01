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

use crate::state::ServerState;

/// Query handler behaviour reachable from crates below `medulla-h-query`.
pub trait QueryApi: Send + Sync {
    /// The semantic owner of an EXISTING binding, for operator reads: unlike opening a
    /// binding it never creates an owner file for a binding that was never admitted.
    #[cfg(feature = "ann-redb")]
    fn existing_semantic_service(
        &self,
        persist_dir: &std::path::Path,
        tenant: &str,
        binding_id: &str,
    ) -> Result<Arc<medulla_core::compute::semantic_index_service::SemanticIndexService>, String>;

    /// Run `plan` over a staged (transaction-overlaid) view with the core indexes, committed
    /// and staged embeddings, and the caller's governed foreign and time-series legs bound,
    /// projecting the rows as `[id, score|nil]`.
    #[cfg(feature = "query")]
    fn run_unified_staged_rows(
        &self,
        plan: medulla_plan::Plan,
        view: &medulla_core::graph::GraphView,
        core: &Arc<medulla_core::graph::GraphCore>,
        staged: &[(String, Vec<f32>)],
        #[cfg(feature = "federation")] foreign: Option<
            &medulla_plan::federation::ForeignSourceRegistry,
        >,
        #[cfg(feature = "tsdb")] tsdb_ctx: query_support::TsdbLegBind<'_>,
    ) -> Result<Vec<(String, Option<f32>)>, String>;
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
        verified_context: &crate::auth::VerifiedRequestContext,
        method: medulla_protocol_types::protocol::Method,
    ) -> Result<medulla_protocol_types::protocol::Response, medulla_protocol_types::protocol::Method>;

    /// Land a staged cross-modal transaction durably with an optional replay nonce.
    async fn commit_cross_modal_txn_with_nonce(
        &self,
        state: &Arc<RwLock<ServerState>>,
        request_id: u64,
        caller: Option<&str>,
        coordinator_id: &str,
        txn: crate::txn::GraphTxnState,
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
        authority: &crate::access::CarrierAuthority,
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
        source: &crate::sql_catalog_acl::SqlSourceAuthorityWrite<'_, '_>,
        table_store: &medulla_query::TableStore,
        txn: &mut medulla_query::TableTxn,
        committed_replay: bool,
    ) -> Result<Vec<medulla_query::TableSchema>, String>;

    /// The committed SQL receipt that authorizes owner repair for a crash replay.
    #[cfg(feature = "query")]
    fn committed_sql_replay_receipt(
        &self,
        store: &medulla_query::TableStore,
        authority: &crate::access::CarrierAuthority,
        batch: &medulla_types::MutationBatch,
    ) -> Result<Option<medulla_protocol_types::mutation_batch::MutationBatchRecord>, String>;
}
