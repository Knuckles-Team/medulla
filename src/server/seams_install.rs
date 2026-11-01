//! Installs every crate-split seam implementation into the state (MEDULLA-CONTRACT-R056.8).
//!
//! Owned by `medulla-server`, the only crate that sees every handler crate. Each seam lane fills
//! its own slot below; keep one blank line between slots.

use super::ServerState;

/// Install the seam implementations. Called once while the startup composes the state.
#[allow(unused_variables)]
pub fn install_seams(state: &ServerState) {
    // L1 server helpers: none (moved items need no installation).

    // L2 dispatch re-entry.
    let _ = state
        .seams
        .dispatch
        .set(std::sync::Arc::new(super::dispatch::DispatchSeam));

    // L3 ingest, graph, jobs, compute.
    let _ = state.seams.graph.set(std::sync::Arc::new(GraphSeam));
    #[cfg(feature = "jobs")]
    let _ = state
        .seams
        .jobs
        .set(std::sync::Arc::new(super::handlers::jobs::JobsSeam));

    // L4 admin, decide.
    let _ = state
        .seams
        .admin
        .set(std::sync::Arc::new(super::handlers::admin::AdminSeam));
    let _ = state
        .seams
        .decide
        .set(std::sync::Arc::new(super::handlers::decide::DecideSeam));

    // L5 query, txn, wire.
    #[cfg(all(feature = "mining", feature = "server"))]
    medulla_kernel::kernel::persistence::install_mining_replay(
        crate::server::handlers::mining::replay,
    );

    #[cfg(any(
        feature = "query",
        feature = "cypher",
        feature = "graphql",
        feature = "mining"
    ))]
    let _ = state.seams.query.set(std::sync::Arc::new(
        crate::server::handlers::query::QuerySeam,
    ));
    let _ = state
        .seams
        .txn
        .set(std::sync::Arc::new(crate::server::handlers::txn::TxnSeam));
    #[cfg(feature = "query")]
    let _ = state
        .seams
        .wire
        .set(std::sync::Arc::new(crate::server::wire::WireSeam));

    // L6 persistence, raft, server-api.
}

/// Graph handler behaviour installed on the seam registry's `graph` slot.
struct GraphSeam;

#[async_trait::async_trait]
impl medulla_server_api::api::ingest::GraphApi for GraphSeam {
    async fn graph_ops_try_handle(
        &self,
        state: &std::sync::Arc<tokio::sync::RwLock<ServerState>>,
        req_id: u64,
        caller: Option<&str>,
        graph_name: &str,
        read_authority: &medulla_server_api::access::GraphReadAuthority,
        core: std::sync::Arc<medulla_graph_core::graph::GraphCore>,
        method: crate::protocol::Method,
    ) -> crate::protocol::Response {
        medulla_h_graph::graph_ops::try_handle(
            state,
            req_id,
            caller,
            graph_name,
            read_authority,
            core,
            method,
        )
        .await
    }
}
