//! medulla-persistence-tests: the redb backend, shard migration, backup, cold-offload and cost tests.
//!
//! Level F5b in `scripts/crate_split/boundaries.toml` (MEDULLA-CONTRACT-R056.11). Every module is a
//! test module: the crate is empty outside the test build. It is a dev-dependency only and turns on the
//! persistence `test-support` feature for its own test build.

#![allow(clippy::too_many_arguments, clippy::items_after_test_module, clippy::new_without_default)] // signatures and fixtures carried over from the facade
#![allow(private_interfaces, private_bounds)] // visibility is narrowed by the unreachable_pub pass
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
