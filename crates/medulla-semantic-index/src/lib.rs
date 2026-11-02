//! medulla-semantic-index: Semantic ANN codes and the semantic index service.
//!
//! Layer L3 (MEDULLA-CONTRACT-R056.11); it may depend only
//! on crates of a lower level. Budget: 25,000 lines of Rust. Specs that land here: federated-query-and-reasoning (vector retrieval).

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
