//! medulla-semantic-index: Semantic ANN codes and the semantic index service.
//!
//! Level L3 in `scripts/crate_split/boundaries.toml` (MEDULLA-CONTRACT-R056.11); it may depend only
//! on crates of a lower level. Budget: 25,000 lines of Rust. Specs that land here: federated-query-and-reasoning (vector retrieval).
//! Route new rows with `scripts/crate_split/route_rows.py`.

#![allow(clippy::too_many_arguments, clippy::items_after_test_module, clippy::new_without_default)] // signatures and fixtures carried over from the facade
#![allow(private_interfaces, private_bounds)] // visibility is narrowed by the unreachable_pub pass
#![allow(dead_code)] // moved helper code, as the facade crate did
#![allow(unused_imports)] // glob imports carried over from the facade; the visibility pass prunes them
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
