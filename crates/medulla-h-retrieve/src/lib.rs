//! medulla-h-retrieve: Discover, semantic index and reasoning projection handlers.
//!
//! Layer F9 (MEDULLA-CONTRACT-R056.11); it may depend only
//! on crates of a lower level. Budget: 25,000 lines of Rust. Specs that land here: none yet.

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

// The ambient auth posture these tests run under (MEDULLA-CONTRACT-R056.18).
#[cfg(test)]
use medulla_test_ambient_auth as _;
