//! medulla-server-store-tests: Served mutation, batch, saga, persistence and store run-owner tests across the server API and store crates.
//!
//! Level F11a in `scripts/crate_split/boundaries.toml` (MEDULLA-CONTRACT-R056.11). Every module is a
//! test module: the crate is empty outside the test build.

#![allow(clippy::too_many_arguments, clippy::items_after_test_module, clippy::new_without_default)] // signatures and fixtures carried over from the facade
#![allow(dead_code)] // moved helper code, as the facade crate did
#![allow(unused_imports)] // glob imports carried over from the facade; the visibility pass prunes them
#![recursion_limit = "256"]
#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![cfg(test)]

#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod split_tests;
