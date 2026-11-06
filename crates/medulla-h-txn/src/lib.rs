//! medulla-h-txn: method handlers (txn). Split out of the `medulla-engine` facade (MEDULLA-CONTRACT-R056.11);
//! layer 5 of the crate layering (MEDULLA-CONTRACT-R056).

#![forbid(unsafe_code)]
#![deny(missing_docs)]

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[cfg(any(feature = "compute-dist", feature = "matview"))]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod dist_compute;

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[cfg(feature = "matview")]
pub mod matview_shadow;

#[cfg(feature = "sqlite-file")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod sqlite_file;

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[cfg(feature = "tsdb")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod timeseries;

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod txn;

// The ambient auth posture these tests run under (MEDULLA-CONTRACT-R056.18).
#[cfg(test)]
use medulla_test_ambient_auth as _;
