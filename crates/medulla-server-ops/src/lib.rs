//! medulla-server-ops: Served API handler seams: the admin, decide, ingest, graph, jobs, compute, query, transaction and wire seam traits, their typed accessors and their support modules.
//!
//! Level F4e in `scripts/crate_split/boundaries.toml` (MEDULLA-CONTRACT-R056.11); it may depend only
//! on crates of a lower level. Budget: 25,000 lines of Rust.
//! Route new rows with `scripts/crate_split/route_rows.py`.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod api;
