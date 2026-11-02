//! medulla-h-sparql: SPARQL/RDF and federation handlers.
//!
//! Level F9 in `scripts/crate_split/boundaries.toml` (MEDULLA-CONTRACT-R056.11); it may depend only
//! on crates of a lower level. Budget: 25,000 lines of Rust. Specs that land here: none yet.
//! Route new rows with `scripts/crate_split/route_rows.py`.

#![allow(clippy::too_many_arguments, clippy::items_after_test_module, clippy::new_without_default)] // signatures and fixtures carried over from the facade
#![allow(dead_code)] // moved helper code, as the facade crate did
#![allow(unused_imports)] // glob imports carried over from the facade; the visibility pass prunes them
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
/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[cfg(feature = "rdf")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod rdf;
/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[cfg(feature = "sparql-fuseki")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod sparql_service;
