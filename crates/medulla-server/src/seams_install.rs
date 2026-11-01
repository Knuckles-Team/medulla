//! Installs every crate-split seam implementation into the state (MEDULLA-CONTRACT-R056.8).
//!
//! Owned by `medulla-server`, the only crate that sees every handler crate. Each seam lane fills
//! its own slot below; keep one blank line between slots.

use medulla_server_api::state::ServerState;

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
    let _ = medulla_server_ops::api::ingest::install_graph_seam(
        &state.seams,
        std::sync::Arc::new(GraphSeam));
    #[cfg(feature = "jobs")]
    let _ = medulla_server_ops::api::ingest::install_jobs_seam(
        &state.seams,
        std::sync::Arc::new(medulla_h_jobs::jobs::JobsSeam));

    // L4 admin, decide.
    let _ = medulla_server_ops::api::admin::install_admin_seam(
        &state.seams,
        std::sync::Arc::new(medulla_h_admin::admin::AdminSeam));
    let _ = medulla_server_ops::api::admin::install_decide_seam(
        &state.seams,
        std::sync::Arc::new(medulla_h_decide::decide::DecideSeam));

    // L5 query, txn, wire.
    #[cfg(all(feature = "mining", feature = "server"))]
    medulla_kernel::kernel::persistence::install_mining_replay(
        medulla_h_query::mining::replay,
    );

    #[cfg(any(
        feature = "query",
        feature = "cypher",
        feature = "graphql",
        feature = "mining"
    ))]
    let _ = medulla_server_ops::api::query_txn::install_query_seam(
        &state.seams,
        std::sync::Arc::new(
        medulla_h_query::query::QuerySeam,
    ));
    let _ = medulla_server_ops::api::query_txn::install_txn_seam(
        &state.seams,
        std::sync::Arc::new(medulla_h_txn::txn::TxnSeam));
    #[cfg(feature = "query")]
    let _ = medulla_server_ops::api::query_txn::install_wire_seam(
        &state.seams,
        std::sync::Arc::new(medulla_wire::wire::WireSeam));

    // L6 persistence, raft, server-api.
}

/// Graph handler behaviour installed on the seam registry's `graph` slot.
struct GraphSeam;

#[async_trait::async_trait]
impl medulla_server_ops::api::ingest::GraphApi for GraphSeam {
    async fn graph_ops_try_handle(
        &self,
        state: &std::sync::Arc<tokio::sync::RwLock<ServerState>>,
        req_id: u64,
        caller: Option<&str>,
        graph_name: &str,
        read_authority: &medulla_server_api::carrier::GraphReadAuthority,
        core: std::sync::Arc<medulla_graph_core::graph::GraphCore>,
        method: medulla_protocol_types::protocol::Method,
    ) -> medulla_protocol_types::protocol::Response {
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
