//! medulla-decision-outcome-types: Decision outcome and governance types: answer ranking, guards, trust, triage, red team, promotion, recommendations, policy evolution and the current plan.
//!
//! Level L1f in `scripts/crate_split/boundaries.toml` (MEDULLA-CONTRACT-R056.11); it may depend only
//! on crates earlier in `[types_split].chain`. Budget: 25,000 lines of Rust.
//! Route new rows with `scripts/crate_split/route_rows.py`.

#![allow(dead_code)] // moved helper code, as the facade crate did
#![forbid(unsafe_code)]
#![deny(missing_docs)]

// The general bounded 0-1 integer programme (`Method::Solve`): model, config,
// certificate and exact scalars. The search and the verifier live in
// `medulla-compute`, which re-exports these so there is one definition of each.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod current_plan;
// EH-346/EH-347 — capture-first open-weight policy evolution: the capability,
// capture, model-policy version, training-run and evaluation records EG owns.
// Pure serde; EG records and relates, it never trains.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod policy_evolution;

#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod decision;
