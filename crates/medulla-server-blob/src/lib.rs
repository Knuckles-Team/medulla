//! medulla-server-blob: Served blob and key-value storage.
//!
//! Layer F4 (MEDULLA-CONTRACT-R056.11); it may depend only
//! on crates of a lower level. Budget: 25,000 lines of Rust. Specs that land here: object-storage-backend (served blobs).

#![forbid(unsafe_code)]
#![deny(missing_docs)]

#[cfg(feature = "blob")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod blob;
/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[cfg(feature = "kv")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod kv;
/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[cfg(feature = "sparql-http")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod sparql_http;

// The ambient auth posture these tests run under (MEDULLA-CONTRACT-R056.18).
#[cfg(test)]
use medulla_test_ambient_auth as _;
