//! medulla-index-types: Semantic index, embedding, managed index and entity resolution types.
//!
//! Level L1 in `scripts/crate_split/boundaries.toml` (MEDULLA-CONTRACT-R056.11); it may depend only
//! on crates earlier in `[types_split].chain`. Budget: 25,000 lines of Rust. Specs that land here: federated-query-and-reasoning (index types).
//! Route new rows with `scripts/crate_split/route_rows.py`.

#![allow(clippy::too_many_arguments, clippy::items_after_test_module, clippy::new_without_default)] // signatures and fixtures carried over from the facade
#![allow(private_interfaces, private_bounds)] // visibility is narrowed by the unreachable_pub pass
#![allow(dead_code)] // moved helper code, as the facade crate did
#![allow(unused_imports)] // glob imports carried over from the facade; the visibility pass prunes them
#![forbid(unsafe_code)]
#![deny(missing_docs)]

// MEDULLA-UNIFIED-DATA-PLANE-R030.2/.3/.700 — blocking, Fellegi-Sunter scoring,
// approval and identity evidence over entity link proposals.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod entity_resolution;
// CONCEPT:MEDULLA-KG.sharding.semantic-embedding-store-backed — the pinned embedding-space
// identity (`EmbeddingSpaceRef`) + stamped-vector (`StampedVector`) currency shared
// by BOTH `medulla-core::compute::semantic` backends, plus their two dimensionality
// ceilings. Pure serde, no dep — unconditional like `commit_descriptor`/
// `row_predicate`, since `medulla-core` needs it on both sides of its backend `#[cfg]`
// split.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod embedding;
/// User-managed indexes and edge-native search on the wire (EH-351 / EH-352).
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod managed_index;
// RF-019 — the sole transport-neutral semantic-index contract.  Runtime
// storage, queues, handlers, and surface projections live in crates above this
// bottom-of-DAG owner and must consume these exact operation and identity DTOs.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod semantic_index;
