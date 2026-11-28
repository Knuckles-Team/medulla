//! medulla-h-admin-schema: method handlers (admin). Split out of the `medulla-engine` facade (MEDULLA-CONTRACT-R056.11);
//! layer 5 of the crate layering (MEDULLA-CONTRACT-R056).

#![forbid(unsafe_code)]
#![deny(missing_docs)]

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod admin;

/// The Agent Library projection worker (MEDULLA-ORCH-VOCAB-R003.6.1).
pub mod agent_library_projection;

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod connector_pack_projection;

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[cfg(feature = "shacl")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod graph_schema;

/// The knowledge bundle handlers (MEDULLA-DURABLE-KERNEL-R877.2).
#[cfg(all(feature = "rdf", feature = "blob", feature = "redb"))]
pub mod knowledge_bundle;

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod namespace_migration;

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod raft_admin;

// The ambient auth posture these tests run under (MEDULLA-CONTRACT-R056.18).
#[cfg(test)]
use medulla_test_ambient_auth as _;
