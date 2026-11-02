//! medulla-ann-codes: Semantic ANN code books: quantisation, encode and decode.
//!
//! Level L3 in `scripts/crate_split/boundaries.toml` (MEDULLA-CONTRACT-R056.11); it may depend only
//! on crates of a lower level. Budget: 25,000 lines of Rust. Specs that land here: none yet.
//! Route new rows with `scripts/crate_split/route_rows.py`.

#![allow(clippy::too_many_arguments, clippy::items_after_test_module, clippy::new_without_default)] // signatures and fixtures carried over from the facade
#![allow(dead_code)] // moved helper code, as the facade crate did
#![allow(unused_imports)] // glob imports carried over from the facade; the visibility pass prunes them
#![forbid(unsafe_code)]
#![deny(missing_docs)]

/// Durable ANN code tier: one index generation's buffers in the `medulla_ann` owner
/// table of `OwnerLayout::SemanticIndex`, written as one admitted
/// `Native(SemanticIndex)` mutation (RF-RULING-007).
#[cfg(feature = "ann-redb")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod semantic_ann_codes;
/// RF-RULING-007 layer (c): the call-graph gate over the semantic regions. Test-only.
#[cfg(test)]
mod semantic_arch_gate;
/// RF-019's callable semantic service: the S1-S6 tiered ingestion orchestration
/// over the durable ANN code tier above.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod semantic_index_service;
#[cfg(all(test, feature = "ann-redb"))]
pub(crate) mod test_scope_grant;
