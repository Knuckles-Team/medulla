//! medulla-decision-types: Decision ladder vocabulary: ResolutionKind, evidence classes, decision requests, answers and revision records.
//!
//! Layer L1 (MEDULLA-CONTRACT-R056.11); it may depend only
//! on crates earlier in `[types_split].chain`. Budget: 25,000 lines of Rust. Specs that land here: decision-engine (new or changed types).

#![forbid(unsafe_code)]
#![deny(missing_docs)]

// MEDULLA-DECISION-ENGINE-R700.1 — belief revision wire types.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod belief_revision;
// MEDULLA-DECISION-ENGINE-R652.1 — research claims, evidence and sealed artifacts.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod claims;
// MEDULLA-DECISION-ENGINE-R152.1 — the `ContextFacts` wire types, shared with the protocol
// crate (a wire type, not a decision type), and the evidence class they cite.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod context_facts;
// MEDULLA-DECISION-ENGINE-R702.1 — Learn-stage correction records and effects.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod correction;
// RF-ADR-010 — the Decide layer's wire contract: assembly requests and records,
// the decision policy, the statistical surface and the two admin jobs. Pure
// data; every algorithm lives above this crate.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod decision;
#[cfg(any(test, feature = "test-support"))]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod decision_fixtures;
// MEDULLA-DECISION-ENGINE-R701.1 — failure records, patterns and gap topics.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod failure_patterns;
// MEDULLA-DECISION-ENGINE-R653.1 — improvement-loop records and the `Loops` op body.
pub mod knowledge_flag;
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod loops;
/// Generated decision ontology and shapes (MEDULLA-DECISION-ENGINE-R054.1).
#[cfg(feature = "contract-schema")]
pub mod ontology;
pub mod participant_memory;
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod solve;
// MEDULLA-DECISION-ENGINE-R153.1 — source table of contents and line index.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod source_toc;
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod usage;
