//! medulla-persistence-tests: the redb backend, shard migration, backup, cold-offload and cost tests.
//!
//! Level F5b in `scripts/crate_split/boundaries.toml` (MEDULLA-CONTRACT-R056.11). Every module is a
//! test module: the crate is empty outside the test build. It is a dev-dependency only and turns on the
//! persistence `test-support` feature for its own test build.

#![allow(dead_code)] // moved helper code, as the facade crate did
#![allow(unused_imports)] // glob imports carried over from the facade; the visibility pass prunes them
#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![cfg(test)]

#[cfg(feature = "redb")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
mod backup_tests;
#[cfg(feature = "redb")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
mod cold_offload_tests;
#[cfg(feature = "cost")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
mod cost_tests;
#[cfg(feature = "redb")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
mod redb_backend_tests;
#[cfg(feature = "redb")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
mod shard_migrate_tests;
