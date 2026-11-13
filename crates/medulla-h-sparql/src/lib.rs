//! medulla-h-sparql: SPARQL/RDF and federation handlers.
//!
//! Layer F9 (MEDULLA-CONTRACT-R056.11); it may depend only
//! on crates of a lower level. Budget: 25,000 lines of Rust. Specs that land here: none yet.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[cfg(feature = "federation-search")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod federation;
/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[cfg(feature = "federation")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod foreign_source_handlers;
/// The served cross-source competency questions (MEDULLA-CQ-XSRC-R008.4).
#[cfg(feature = "rdf")]
pub mod cross_source_question;
/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[cfg(feature = "rdf")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod rdf;
/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
#[cfg(feature = "sparql-fuseki")]
pub mod sparql_service;

// The ambient auth posture these tests run under (MEDULLA-CONTRACT-R056.18).
#[cfg(test)]
use medulla_test_ambient_auth as _;
