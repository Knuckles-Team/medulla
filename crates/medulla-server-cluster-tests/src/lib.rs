//! medulla-server-cluster-tests: Cluster tests: raft clusters and commands, cross-shard harness, placement and resource acceptance.
//!
//! Level F11a in `scripts/crate_split/boundaries.toml` (MEDULLA-CONTRACT-R056.11). Every module is a
//! test module: the crate is empty outside the test build.

#![allow(clippy::too_many_arguments, clippy::items_after_test_module, clippy::new_without_default)] // signatures and fixtures carried over from the facade
#![allow(private_interfaces, private_bounds)] // visibility is narrowed by the unreachable_pub pass
#![allow(dead_code)] // moved helper code, as the facade crate did
#![allow(unused_imports)] // glob imports carried over from the facade; the visibility pass prunes them
#![recursion_limit = "256"]
#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![cfg(test)]

#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod split_tests;
