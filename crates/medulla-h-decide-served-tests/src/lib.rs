//! medulla-h-decide-served-tests: method handlers (decide). Split out of the `medulla-engine` facade (MEDULLA-CONTRACT-R056.11);
//! layer 5 of the crate layering (MEDULLA-CONTRACT-R056).
//!
//! The third test crate of the Decide split (MEDULLA-CONTRACT-R067.7): served tests of the decision families and jobs, built on the shared helpers of `medulla-h-decide-stat-tests`. Empty outside the test build.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod decide;

// The ambient auth posture these tests run under (MEDULLA-CONTRACT-R056.18).
#[cfg(test)]
use medulla_test_ambient_auth as _;
