//! medulla-server-identity: Identity store dispatch, request boundary and policy evolution dispatch.
//!
//! Level F10 in `scripts/crate_split/boundaries.toml` (MEDULLA-CONTRACT-R056.11); it may depend only
//! on crates of a lower level. Budget: 25,000 lines of Rust. Specs that land here: none yet.
//! Route new rows with `scripts/crate_split/route_rows.py`.

#![allow(dead_code)] // moved helper code, as the facade crate did
#![recursion_limit = "256"]
#![forbid(unsafe_code)]
#![deny(missing_docs)]

#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod dispatch;
