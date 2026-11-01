//! medulla-work-store: Durable work items and their capability records.
//!
//! Level F2 in `scripts/crate_split/boundaries.toml` (MEDULLA-CONTRACT-R056.11); it may depend only
//! on crates of a lower level. Budget: 25,000 lines of Rust. Specs that land here: orchestration-vocabulary (work item store).
//! Route new rows with `scripts/crate_split/route_rows.py`.

#![recursion_limit = "256"]
#![forbid(unsafe_code)]
#![deny(missing_docs)]

#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod redb_store;
