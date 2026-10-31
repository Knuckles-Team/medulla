//! medulla-reasoner: OWL, tableau and rule reasoning plus reasoning closure.
//!
//! Level L3 in `scripts/crate_split/boundaries.toml` (MEDULLA-CONTRACT-R056.11); it may depend only
//! on crates of a lower level. Budget: 25,000 lines of Rust. Specs that land here: federated-query-and-reasoning (reasoning), durable-graph-kernel (forward chaining).
//! Route new rows with `scripts/crate_split/route_rows.py`.

#![forbid(unsafe_code)]
#![deny(missing_docs)]
