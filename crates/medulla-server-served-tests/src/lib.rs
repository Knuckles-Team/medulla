//! medulla-server-served-tests: End-to-end served tests: the full dispatch chain, handler and wire protocol round trips through the assembled server.
//!
//! Level F11a in `scripts/crate_split/boundaries.toml` (MEDULLA-CONTRACT-R056.11). Every module is a
//! test module: the crate is empty outside the test build.

#![allow(dead_code)] // moved helper code, as the facade crate did
#![recursion_limit = "256"]
#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![cfg(test)]

#[cfg(test)]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
mod tests;

#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod split_tests;
