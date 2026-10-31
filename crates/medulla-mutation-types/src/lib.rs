//! medulla-mutation-types: Mutation, batch, change envelope, commit descriptor, outbox, lease and consensus record types.
//!
//! Level L1 in `scripts/crate_split/boundaries.toml` (MEDULLA-CONTRACT-R056.11); it may depend only
//! on crates of a lower level. Budget: 25,000 lines of Rust. Specs that land here: durable-graph-kernel (types).
//! Route new rows with `scripts/crate_split/route_rows.py`.

#![forbid(unsafe_code)]
#![deny(missing_docs)]
