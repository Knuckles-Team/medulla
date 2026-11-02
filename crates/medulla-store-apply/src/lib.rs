//! medulla-store-apply: Store apply composition: native apply dispatch over work and resource rows, control, cleanup, checkpoints, batch commit and the whole-batch store tests.
//!
//! Level F2b in `scripts/crate_split/boundaries.toml` (MEDULLA-CONTRACT-R056.11); it may depend only
//! on crates of a lower level. Budget: 25,000 lines of Rust.

#![allow(clippy::too_many_arguments)] // seam signatures mirror the moved facade functions
#![allow(dead_code)] // moved helper code, as the facade crate did
#![allow(unused_imports)] // glob imports carried over from the facade; the visibility pass prunes them
#![recursion_limit = "256"]
#![forbid(unsafe_code)]
#![deny(missing_docs)]

#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod redb_store;
