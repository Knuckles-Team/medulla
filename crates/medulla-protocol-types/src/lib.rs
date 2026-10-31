//! medulla-protocol-types: Method registry and wire-query envelope types shared by every listener and client.
//!
//! Level L1 in `scripts/crate_split/boundaries.toml` (MEDULLA-CONTRACT-R056.11); it may depend only
//! on crates of a lower level. Budget: 25,000 lines of Rust. Specs that land here: public-engine-contract-and-release (types).
//! Route new rows with `scripts/crate_split/route_rows.py`.

#![forbid(unsafe_code)]
#![deny(missing_docs)]
