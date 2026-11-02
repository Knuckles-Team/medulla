//! Modules moved here from `medulla_server::split_tests` (hard cutover, no re-exports).

#[cfg(all(test, feature = "raft", feature = "harness"))]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
mod placement_harness;
#[cfg(all(test, feature = "raft", feature = "harness"))]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
mod raft_cluster;
#[cfg(all(test, feature = "raft", feature = "harness"))]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
mod raft_command;
#[cfg(all(test, feature = "raft", feature = "harness"))]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
mod resource_acceptance_test;
#[cfg(all(test, feature = "raft", feature = "harness"))]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
mod xshard_harness;
