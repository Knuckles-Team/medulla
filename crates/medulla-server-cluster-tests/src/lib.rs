//! medulla-server-cluster-tests: Cluster tests: raft clusters and commands, cross-shard harness, placement and resource acceptance.
//!
//! Layer F11a (MEDULLA-CONTRACT-R056.11). Every module is a
//! test module: the crate is empty outside the test build.

#![recursion_limit = "256"]
#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![cfg(test)]

#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod split_tests;

// The ambient auth posture these tests run under (MEDULLA-CONTRACT-R056.18).
#[cfg(test)]
use medulla_test_ambient_auth as _;
