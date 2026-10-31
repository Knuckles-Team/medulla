//! medulla-decision-eval: Decision evaluation: served proof suites, quality and relevance scoring, drift and nightly evaluation.
//!
//! Level L6 in `scripts/crate_split/boundaries.toml` (MEDULLA-CONTRACT-R056.11); it may depend only
//! on crates of a lower level. Budget: 25,000 lines of Rust. Specs that land here: decision-engine (evaluation and proof).
//! Route new rows with `scripts/crate_split/route_rows.py`.

#![forbid(unsafe_code)]
#![deny(missing_docs)]
