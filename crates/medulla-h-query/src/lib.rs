//! medulla-h-query: method handlers (query). Split out of the `medulla-engine` facade (MEDULLA-CONTRACT-R056.11);
//! level 5 in `scripts/crate_split/manifest.toml`.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[cfg(feature = "federation")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod federation;

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[cfg(feature = "knowledge-batch")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod knowledge_stream;

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[cfg(feature = "mining")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod mining;

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[cfg(feature = "nl-query")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod nl;

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[cfg(any(feature = "query", feature = "cypher", feature = "graphql"))]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod query;

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[cfg(feature = "rdf")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod rdf;

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[cfg(feature = "epistemic-tms")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod reasoning_projection;

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[cfg(feature = "query")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod request_cancel;

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[cfg(any(feature = "text", feature = "tsdb", feature = "owl", feature = "geo"))]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod secondary_indexes;

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[cfg(feature = "ann-redb")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod semantic_index;

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[cfg(feature = "sparql-fuseki")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod sparql_service;

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[cfg(feature = "streaming")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod streaming;
