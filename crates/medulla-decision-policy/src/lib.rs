//! medulla-decision-policy: Ladder stages 3-5: policy, hard constraints, knobs, admission, refusals, budgets and the bounded exact solve.
//!
//! Level L4 in `scripts/crate_split/boundaries.toml` (MEDULLA-CONTRACT-R056.11); it may depend only
//! on crates of a lower level. Budget: 25,000 lines of Rust. Specs that land here: decision-engine (policy and constraints).
//! Route new rows with `scripts/crate_split/route_rows.py`.

#![allow(dead_code)] // moved helper code, as the facade crate did
#![allow(unused_imports)] // glob imports carried over from the facade; the visibility pass prunes them
#![forbid(unsafe_code)]
#![deny(missing_docs)]

// Decide layer package A3 — general bounded 0-1 integer programming: validated
// models, deterministic node-budgeted branch-and-bound, Lagrangian leaf-bound
// certificates and an independent verifier. Integer-only, no heavy deps.
#[cfg(feature = "solve")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod solve;
