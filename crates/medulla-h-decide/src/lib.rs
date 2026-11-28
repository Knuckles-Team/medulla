//! medulla-h-decide: method handlers (decide). Split out of the `medulla-engine` facade (MEDULLA-CONTRACT-R056.11);
//! layer 5 of the crate layering (MEDULLA-CONTRACT-R056).
//!
//! The top handler crate of the Decide split (MEDULLA-CONTRACT-R067.7): the served `Decide` entry, the decision log's served entry points and replay, the commit, the loops, the family registry and the `DecideApi` seam. It builds on the three lower handler crates.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod decide;

/// The served per-tenant usage ledger (MEDULLA-DECISION-ENGINE-R801.8).
pub mod usage;

// The ambient auth posture these tests run under (MEDULLA-CONTRACT-R056.18).
#[cfg(test)]
use medulla_test_ambient_auth as _;
