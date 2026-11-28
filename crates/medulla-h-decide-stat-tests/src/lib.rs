//! medulla-h-decide-stat-tests: method handlers (decide). Split out of the `medulla-engine` facade (MEDULLA-CONTRACT-R056.11);
//! layer 5 of the crate layering (MEDULLA-CONTRACT-R056).
//!
//! The first test crate of the Decide split (MEDULLA-CONTRACT-R067.7): the shared helpers `decide::stat_tests` and the test modules the other two test crates name, compiled for them behind the `test-support` feature, plus its own test modules. Empty outside the test build.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod decide;

// The ambient auth posture these tests run under (MEDULLA-CONTRACT-R056.18).
#[cfg(test)]
use medulla_test_ambient_auth as _;
