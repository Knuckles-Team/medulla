//! medulla-h-retrieve: Discover, semantic index and reasoning projection handlers.
//!
//! Level F9 in `scripts/crate_split/boundaries.toml` (MEDULLA-CONTRACT-R056.11); it may depend only
//! on crates of a lower level. Budget: 25,000 lines of Rust. Specs that land here: none yet.
//! Route new rows with `scripts/crate_split/route_rows.py`.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[cfg(feature = "epistemic-tms")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod reasoning_projection;
/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[cfg(any(feature = "text", feature = "tsdb", feature = "owl", feature = "geo"))]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod secondary_indexes;
/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[cfg(feature = "ann-redb")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod semantic_index;
/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[cfg(feature = "ann-redb")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod semantic_index_runtime;
