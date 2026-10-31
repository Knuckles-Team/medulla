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

    // L4 admin, decide.

    // L5 query, txn, wire.
    #[cfg(any(feature = "query", feature = "cypher", feature = "graphql"))]
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
