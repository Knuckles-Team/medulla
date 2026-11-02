//! medulla-decision-types: Decision ladder vocabulary: ResolutionKind, evidence classes, decision requests, answers and revision records.
//!
//! Level L1 in `scripts/crate_split/boundaries.toml` (MEDULLA-CONTRACT-R056.11); it may depend only
//! on crates earlier in `[types_split].chain`. Budget: 25,000 lines of Rust. Specs that land here: decision-engine (new or changed types).
//! Route new rows with `scripts/crate_split/route_rows.py`.

#![allow(dead_code)] // moved helper code, as the facade crate did
#![allow(unused_imports)] // glob imports carried over from the facade; the visibility pass prunes them
#![forbid(unsafe_code)]
#![deny(missing_docs)]

// RF-ADR-010 — the Decide layer's wire contract: assembly requests and records,
// the decision policy, the statistical surface and the two admin jobs. Pure
// data; every algorithm lives above this crate.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod decision;
#[cfg(any(test, feature = "test-support"))]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod decision_fixtures;
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod solve;
