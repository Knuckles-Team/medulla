//! medulla-reasoner: OWL, tableau and rule reasoning plus reasoning closure.
//!
//! Level L3 in `scripts/crate_split/boundaries.toml` (MEDULLA-CONTRACT-R056.11); it may depend only
//! on crates of a lower level. Budget: 25,000 lines of Rust. Specs that land here: federated-query-and-reasoning (reasoning), durable-graph-kernel (forward chaining).
//! Route new rows with `scripts/crate_split/route_rows.py`.

#![allow(dead_code)] // moved helper code, as the facade crate did
#![allow(unused_imports)] // glob imports carried over from the facade; the visibility pass prunes them
#![deny(unsafe_code)]
#![deny(missing_docs)]

/// W3 — the native OWL 2 (EL⁺ + RL) reasoner (CONCEPT:MEDULLA-KG.ontology.incremental-materialization). Classification,
/// consistency checking, incremental materialization + justifications over the
/// oxttl-parsed ontology. Pure Rust; behind `owl` (implies `rdf`).
#[cfg(feature = "owl")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod owl;
/// The wire projection of an OWL proof tree (`ProofNode` → `ProofNodeWire`), shared by
/// `OwlExplain` and UQL `WITH PROOF`.
#[cfg(feature = "owl")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod owl_wire;
/// ModalityContract retrofit (CONCEPT:E4): `impl ModalityContract for
/// owl::ProofNode` — the reference non-trivial `provenance()`, mapping
/// `owl::Justification` (reconstructed as a `ProofNode`) losslessly. Behind the
/// crate's own opt-in `contract` feature (default OFF, implying `owl` since
/// `ProofNode` lives there). See `src/contract.rs` module docs.
#[cfg(feature = "contract")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
mod contract;
/// MEDULLA-021 — user-defined custom rules + instance-level (ABox) OWL 2 RL / Datalog
/// reasoning with `owl:sameAs` equality, run in one confidence-propagating
/// forward-chaining fixpoint. The second Stardog-parity gap (custom rules) plus the
/// instance side of the broader OWL-2 axioms (`owl.rs` covers the TBox/concept side).
/// [`rules::run_rule_reasoning`] / [`rules::run_rule_reasoning_on_view`] are the
/// server-op-ready entry points (a `RunRules` op is a thin pass-through). Behind `owl`.
#[cfg(feature = "owl")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod rules;
/// W5 — the OWL-DL **tableau** reasoner (CONCEPT:MEDULLA-KG.ontology.concept-2). A pure-Rust description-logic
/// tableau (ALCQO + role hierarchy) that DECIDES the constructs the monotone EL⁺/RL
/// [`owl`] path cannot: full cardinality restrictions (`≥n`/`≤n`, qualified),
/// `complementOf`/negation (via negation-normal-form), `oneOf`/`hasValue` nominals, and
/// reasoning-by-cases over a `unionOf` SUPERCLASS. Entry points: [`tableau::is_consistent`],
/// [`tableau::is_subsumed`], [`tableau::is_instance`], [`tableau::classify_dl`], and the
/// [`tableau::reason_dl`] engine-picker (EL⁺/RL fast path when the ontology is inside the
/// tractable envelope, tableau only when a DL construct forces it). Pure Rust; behind
/// `owl-dl` (implies `owl`). Pi contract: OUT of `pi`, folded into node/full.
#[cfg(feature = "owl-dl")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod tableau;
#[cfg(feature = "reasoning")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod reasoning;
#[cfg(feature = "reasoning")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod reasoning_closure;
