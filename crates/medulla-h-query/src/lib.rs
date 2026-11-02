//! medulla-h-query: method handlers (query). Split out of the `medulla-engine` facade (MEDULLA-CONTRACT-R056.11);
//! layer 5 of the crate layering (MEDULLA-CONTRACT-R056).

#![forbid(unsafe_code)]
#![deny(missing_docs)]



/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[cfg(feature = "knowledge-batch")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod knowledge_stream;


/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[cfg(feature = "nl-query")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod nl;

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[cfg(any(feature = "query", feature = "cypher", feature = "graphql"))]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod query;



/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[cfg(feature = "query")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod request_cancel;





/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[cfg(feature = "streaming")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod streaming;

// The ambient auth posture these tests run under (MEDULLA-CONTRACT-R056.18).
#[cfg(test)]
use medulla_test_ambient_auth as _;
