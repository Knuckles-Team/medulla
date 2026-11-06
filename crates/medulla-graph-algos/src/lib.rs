//! medulla-graph-algos: Graph algorithms and graph learning.
//!
//! Layer L4 (MEDULLA-CONTRACT-R056.11); it may depend only
//! on crates of a lower level. Budget: 25,000 lines of Rust. Specs that land here: none yet.

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
/// Graph-guided paging adapter: PageRank / centrality importance scores shaped for a
/// byte-budgeted tiered cache's importance-scored eviction.
pub mod graph_importance;
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
