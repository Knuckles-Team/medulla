//! medulla-decision-stats: Statistical ranking for the decision ladder: fitted heads, calibration, conformal bounds, off-policy evaluation and scores.
//!
//! Level L3 in `scripts/crate_split/boundaries.toml` (MEDULLA-CONTRACT-R056.11); it may depend only
//! on crates of a lower level. Budget: 25,000 lines of Rust. Specs that land here: decision-engine (statistical ranking).
//! Route new rows with `scripts/crate_split/route_rows.py`.

#![forbid(unsafe_code)]
#![deny(missing_docs)]
