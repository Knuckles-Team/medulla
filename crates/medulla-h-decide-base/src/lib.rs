//! medulla-h-decide-base: method handlers (decide). Split out of the `medulla-engine` facade (MEDULLA-CONTRACT-R056.11);
//! layer 5 of the crate layering (MEDULLA-CONTRACT-R056).
//!
//! The lowest of the four handler crates of the Decide split (MEDULLA-CONTRACT-R067.7): the decision executor and its support, the decision-log reader, the pointers and the candidate sources, and the families that need nothing above them.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod decide;

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod solve;

// The ambient auth posture these tests run under (MEDULLA-CONTRACT-R056.18).
#[cfg(test)]
use medulla_test_ambient_auth as _;
