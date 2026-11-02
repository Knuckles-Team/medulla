//! medulla-persistence-tests: the redb backend, shard migration, backup, cold-offload and cost tests.
//!
//! Layer F5b (MEDULLA-CONTRACT-R056.11). Every module is a
//! test module: the crate is empty outside the test build. It is a dev-dependency only and turns on the
//! persistence `test-support` feature for its own test build.

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

// The ambient auth posture these tests run under (MEDULLA-CONTRACT-R056.18).
#[cfg(test)]
use medulla_test_ambient_auth as _;
