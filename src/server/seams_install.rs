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

    // L3 ingest, graph, jobs, compute.

    // L4 admin, decide.

    // L5 query, txn, wire.

    // L6 persistence, raft, server-api.
}
