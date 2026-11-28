//! medulla-decision-program-types: Decision ladder vocabulary: ResolutionKind, evidence classes, decision requests, answers and revision records.
//!
//! Layer L1 (MEDULLA-CONTRACT-R056.11); it may depend only
//! on crates earlier in `[types_split].chain`. Budget: 25,000 lines of Rust. Specs that land here: decision-engine (new or changed types).

#![forbid(unsafe_code)]
#![deny(missing_docs)]

// RF-ADR-010 — the Decide layer's wire contract: assembly requests and records,
// the decision policy, the statistical surface and the two admin jobs. Pure
// data; every algorithm lives above this crate.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod decision;
// MEDULLA-DECISION-ENGINE-R653.1 — improvement-loop records and the `Loops` op body.
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod friction;
