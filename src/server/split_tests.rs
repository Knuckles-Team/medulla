//! Tests that need the full server (router, seam installation, redb backend).
//!
//! These fixtures drive `dispatch` or the concrete persistence backends, so they sit in the
//! top server crate rather than beside the code they exercise (R056.8).

#[cfg(all(test, feature = "redb"))]
mod admin_agent_library_security;
#[cfg(all(test, feature = "raft", feature = "harness"))]
mod placement_harness;
#[cfg(all(test, feature = "raft"))]
mod resource_acceptance_test;
#[cfg(all(test, feature = "redb"))]
mod server_api_saga;
#[cfg(test)]
mod store_run_owner;
#[cfg(all(test, any(feature = "server", feature = "redb")))]
mod store_terminal_batch;
#[cfg(test)]
mod wired_edge_index;
#[cfg(all(test, feature = "raft", feature = "harness"))]
mod xshard_harness;
#[cfg(test)]
mod impact_watch;
#[cfg(all(test, feature = "redb"))]
mod mutation_batch_internal_replay;
#[cfg(all(test, feature = "redb"))]
mod mutation_batch_lifecycle;
#[cfg(all(test, feature = "redb"))]
mod mutation_redb;
