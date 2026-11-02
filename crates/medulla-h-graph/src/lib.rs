//! medulla-h-graph: method handlers (graph). Split out of the `medulla-engine` facade (MEDULLA-CONTRACT-R056.11);
//! layer 5 of the crate layering (MEDULLA-CONTRACT-R056).

#![forbid(unsafe_code)]
#![deny(missing_docs)]

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod graph_ops;

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod harness_admission;

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[cfg(feature = "ml-pipeline")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod pipeline;

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod placement;

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod topology;

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[cfg(feature = "redb")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod write_back;

// The ambient auth posture these tests run under (MEDULLA-CONTRACT-R056.18).
#[cfg(test)]
use medulla_test_ambient_auth as _;
