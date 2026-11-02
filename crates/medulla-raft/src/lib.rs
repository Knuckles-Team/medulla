//! medulla-raft: consensus and replication. Split out of the `medulla-engine` facade (MEDULLA-CONTRACT-R056.11);
//! layer 4 of the crate layering (MEDULLA-CONTRACT-R056).

#![forbid(unsafe_code)]
#![deny(missing_docs)]

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[cfg(feature = "raft")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod raft;

// The ambient auth posture these tests run under (MEDULLA-CONTRACT-R056.18).
#[cfg(test)]
use medulla_test_ambient_auth as _;
