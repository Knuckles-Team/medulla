//! medulla-decision-types: Decision ladder vocabulary: ResolutionKind, evidence classes, decision requests, answers and revision records.
//!
//! Level L1 in `scripts/crate_split/boundaries.toml` (MEDULLA-CONTRACT-R056.11); it may depend only
//! on crates of a lower level. Budget: 25,000 lines of Rust. Specs that land here: decision-engine (new or changed types).
//! Route new rows with `scripts/crate_split/route_rows.py`.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

// RF-ADR-010 — the Decide layer's wire contract: assembly requests and records,
// the decision policy, the statistical surface and the two admin jobs. Pure
// data; every algorithm lives above this crate.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod decision;
// EH-346/EH-347 — capture-first open-weight policy evolution: the capability,
// capture, model-policy version, training-run and evaluation records EG owns.
// Pure serde; EG records and relates, it never trains.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod policy_evolution;
// The general bounded 0-1 integer programme (`Method::Solve`): model, config,
// certificate and exact scalars. The search and the verifier live in
// `medulla-compute`, which re-exports these so there is one definition of each.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod current_plan;
#[cfg(any(test, feature = "test-support"))]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod decision_fixtures;
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod solve;
