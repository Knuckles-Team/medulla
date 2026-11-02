//! medulla-h-query: method handlers (query). Split out of the `medulla-engine` facade (MEDULLA-CONTRACT-R056.11);
//! level 5 in `scripts/crate_split/manifest.toml`.

#![allow(clippy::too_many_arguments, clippy::items_after_test_module, clippy::new_without_default)] // signatures and fixtures carried over from the facade
#![allow(dead_code)] // moved helper code, as the facade crate did
#![allow(unused_imports)] // glob imports carried over from the facade; the visibility pass prunes them
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
