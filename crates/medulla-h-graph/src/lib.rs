//! medulla-h-graph: method handlers (graph). Split out of the `medulla-engine` facade (MEDULLA-CONTRACT-R056.11);
//! layer 5 of the crate layering (MEDULLA-CONTRACT-R056).

#![forbid(unsafe_code)]
#![deny(missing_docs)]

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod graph_ops;

/// MEDULLA-REPO-INGEST-R002.651.2: the supersession check of one change envelope.
pub mod supersession_admission;

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
