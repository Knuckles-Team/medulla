//! medulla-decision-policy: Ladder stages 3-5: policy, hard constraints, knobs, admission, refusals, budgets and the bounded exact solve.
//!
//! Level L4 in `scripts/crate_split/boundaries.toml` (MEDULLA-CONTRACT-R056.11); it may depend only
//! on crates of a lower level. Budget: 25,000 lines of Rust. Specs that land here: decision-engine (policy and constraints).
//! Route new rows with `scripts/crate_split/route_rows.py`.

#![forbid(unsafe_code)]
#![deny(missing_docs)]
