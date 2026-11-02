//! medulla-semantic-index: Semantic ANN codes and the semantic index service.
//!
//! Level L3 in `scripts/crate_split/boundaries.toml` (MEDULLA-CONTRACT-R056.11); it may depend only
//! on crates of a lower level. Budget: 25,000 lines of Rust. Specs that land here: federated-query-and-reasoning (vector retrieval).
//! Route new rows with `scripts/crate_split/route_rows.py`.

#![allow(dead_code)] // moved helper code, as the facade crate did
#![forbid(unsafe_code)]
#![deny(missing_docs)]

/// EH-536 — cooperative interruption of long CPU-bound work on behalf of a
/// served request whose client has gone.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod interrupt;
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod semantic;
#[cfg(feature = "ann")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod semantic_ann;
#[cfg(test)]
pub(crate) mod test_threads;
