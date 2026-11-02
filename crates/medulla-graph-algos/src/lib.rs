//! medulla-graph-algos: Graph algorithms and graph learning.
//!
//! Level L4 in `scripts/crate_split/boundaries.toml` (MEDULLA-CONTRACT-R056.11); it may depend only
//! on crates of a lower level. Budget: 25,000 lines of Rust. Specs that land here: none yet.
//! Route new rows with `scripts/crate_split/route_rows.py`.

#![allow(clippy::too_many_arguments, clippy::items_after_test_module, clippy::new_without_default)] // signatures and fixtures carried over from the facade
#![allow(dead_code)] // moved helper code, as the facade crate did
#![allow(unused_imports)] // glob imports carried over from the facade; the visibility pass prunes them
#![forbid(unsafe_code)]
#![deny(missing_docs)]

#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod algorithms;
// ModalityContract retrofit (CONCEPT:E4): `impl ModalityContract for MergeProposal`,
// behind the crate's own opt-in `contract` feature (default OFF). See `src/contract.rs`.
#[cfg(feature = "contract")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
mod contract;
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod graph_algos;
// CONCEPT:MEDULLA-KG.graphlearn.link-predictor — graph-learning / neuro-symbolic domain.
// A pure-Rust KAN (Kolmogorov-Arnold) link-predictor over the resident graph: a
// polynomial-basis learnable edge function (`edge_fn`), a 1–2 layer KAN link-scorer
// over structural features (`link_predict`), and 1-hop neighbor aggregation
// (`neighbor_aggregate`). Graph-agnostic (works over `graph_algos::AdjacencyGraph`),
// so it is unit-testable in isolation. Feature-gated like mining; implies
// `datascience` for the shared Adam/SGD training kernels.
#[cfg(feature = "graphlearn")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod graphlearn;

#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod community;
