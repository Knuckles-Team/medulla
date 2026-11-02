//! medulla-query-cache: Dependency scopes, the version-keyed query-result cache and the projection catalog.
//!
//! Level L3 in `scripts/crate_split/boundaries.toml` (MEDULLA-CONTRACT-R056.11); it may depend only
//! on crates of a lower level. Budget: 25,000 lines of Rust. Specs that land here: none yet.
//! Route new rows with `scripts/crate_split/route_rows.py`.

#![allow(clippy::too_many_arguments, clippy::items_after_test_module, clippy::new_without_default)] // signatures and fixtures carried over from the facade
#![allow(dead_code)] // moved helper code, as the facade crate did
#![allow(unused_imports)] // glob imports carried over from the facade; the visibility pass prunes them
#![forbid(unsafe_code)]
#![deny(missing_docs)]

/// CONCEPT:MEDULLA-KG.coordination.dependency-scoped-cache-invalidation (W1.6/P7) — the per-graph
/// dependency clock backing the result cache's dependency-scoped invalidation: a write
/// bumps only the label / property-key / node / edge dimensions it actually touched, so a
/// cached query result survives every write whose change-set is DISJOINT from the query's
/// dependency set. Gated with the result cache it serves.
#[cfg(feature = "result-cache")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod dep_scope;
/// CONCEPT:MEDULLA-KG.query.named-graph-projection-catalog (W4.5 / N5) — the `gds.graph.project`-
/// equivalent named/materialized graph-projection catalog, invalidated via the SAME `dep_scope`
/// `DepClock` the result cache's dependency-scoped entries use. Gated with it (needs the clock).
#[cfg(feature = "result-cache")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod projection_catalog;
#[cfg(feature = "result-cache")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod result_cache;
