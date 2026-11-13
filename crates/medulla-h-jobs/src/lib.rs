//! medulla-h-jobs: method handlers (jobs). Split out of the `medulla-engine` facade (MEDULLA-CONTRACT-R056.11);
//! layer 5 of the crate layering (MEDULLA-CONTRACT-R056).

#![forbid(unsafe_code)]
#![deny(missing_docs)]

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[cfg(feature = "statechart")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod agent_lifecycle_statechart;

/// MEDULLA-UNIFIED-DATA-PLANE-R700.3: served governed actions.
#[allow(missing_docs)]
pub mod action;

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod delegation;

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod development_lane;

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[cfg(feature = "jobs")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod jobs;

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod mutation_outbox;

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod native_write_authority;

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[cfg(feature = "statechart")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod statechart;

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod work_item;

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[cfg(feature = "redb")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod work_item_read;

// MEDULLA-DECISION-ENGINE-R133.8: terminal commit with a run trace, through dispatch.
#[cfg(all(test, feature = "redb"))]
mod work_item_tests;

/// MEDULLA-DURABLE-KERNEL-R077: served scheduler worker-admission reads.
#[cfg(feature = "redb")]
pub mod worker_admission;

// The ambient auth posture these tests run under (MEDULLA-CONTRACT-R056.18).
#[cfg(test)]
use medulla_test_ambient_auth as _;
