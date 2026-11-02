//! medulla-server-graph: Graph pipeline dispatch, SPARQL update dispatch and change envelopes.
//!
//! Level F10 in `scripts/crate_split/boundaries.toml` (MEDULLA-CONTRACT-R056.11); it may depend only
//! on crates of a lower level. Budget: 25,000 lines of Rust. Specs that land here: none yet.
//! Route new rows with `scripts/crate_split/route_rows.py`.

#![allow(clippy::too_many_arguments, clippy::items_after_test_module, clippy::new_without_default)] // signatures and fixtures carried over from the facade
#![allow(private_interfaces, private_bounds)] // visibility is narrowed by the unreachable_pub pass
#![allow(dead_code)] // moved helper code, as the facade crate did
#![allow(unused_imports)] // glob imports carried over from the facade; the visibility pass prunes them
#![recursion_limit = "256"]
#![forbid(unsafe_code)]
#![deny(missing_docs)]

// Fleet server registry stale-lease reaper (CONCEPT:MEDULLA-KG.sharding.server-registry, W2.5): periodic
// sweep that expires a `:Server` node whose `Method::RegisterServer`-issued
// lease has lapsed. Always declared (mirrors `semantic_activation` above) — the
// sweep is a no-op when nothing has registered.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod handlers;
// Semantic ANN index activation (W0.4, CONCEPT:MEDULLA-KG.storage.semantic-index-directory): the ONE
// reopen-else-build-else-activate body plus the two triggers the boot-time task
// cannot supply — the post-write hook and the periodic backstop — for a graph
// created, or crossing `ANN_BUILD_THRESHOLD`, after boot. Gated with the `ann`
// feature the mechanism requires; its durable tier additionally needs `ann-redb`.
#[cfg(feature = "ann")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod semantic_activation;

#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod dispatch;
