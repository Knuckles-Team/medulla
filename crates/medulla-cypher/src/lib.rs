//! medulla-cypher: Cypher language, its executor, graph data science calls and the edge index.
//!
//! Level L4 in `scripts/crate_split/boundaries.toml` (MEDULLA-CONTRACT-R056.11); it may depend only
//! on crates of a lower level. Budget: 25,000 lines of Rust. Specs that land here: engine-parity (Cypher).
//! Route new rows with `scripts/crate_split/route_rows.py`.

#![allow(dead_code)] // moved helper code, as the facade crate did
#![forbid(unsafe_code)]
#![deny(missing_docs)]

/// Edge-native vector and text search under the user-managed index lifecycle
/// (EH-351 / EH-352). Behind `sql` (it reuses medulla-ann, medulla-text and the SQL
/// vector metrics).
#[cfg(feature = "sql")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod edge_index;
#[cfg(feature = "cypher")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod cypher;
