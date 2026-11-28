//! medulla-h-decide-runlog: method handlers (decide). Split out of the `medulla-engine` facade (MEDULLA-CONTRACT-R056.11);
//! layer 5 of the crate layering (MEDULLA-CONTRACT-R056).
//!
//! The second handler crate of the Decide split (MEDULLA-CONTRACT-R067.7): the assembly entry, the generation cycle, the retrieval and task-plan records, the policy activation and the families they reach. It builds on `medulla-h-decide-base`.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod decide;

// The ambient auth posture these tests run under (MEDULLA-CONTRACT-R056.18).
#[cfg(test)]
use medulla_test_ambient_auth as _;
