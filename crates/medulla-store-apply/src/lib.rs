//! medulla-store-apply: Store apply composition: native apply dispatch over work and resource rows, control, cleanup, checkpoints, batch commit and the whole-batch store tests.
//!
//! Layer F2b (MEDULLA-CONTRACT-R056.11); it may depend only
//! on crates of a lower level. Budget: 25,000 lines of Rust.

#![recursion_limit = "256"]
#![forbid(unsafe_code)]
#![deny(missing_docs)]

#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod redb_store;
