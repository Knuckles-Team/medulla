//! medulla-server-cluster-tests: Cluster tests: raft clusters and commands, cross-shard harness, placement and resource acceptance.
//!
//! Level F11a in `scripts/crate_split/boundaries.toml` (MEDULLA-CONTRACT-R056.11). Every module is a
//! test module: the crate is empty outside the test build.

#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![cfg(test)]
