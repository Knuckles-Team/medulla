//! medulla-table-store: Physical table store under the table model.
//!
//! Level L4 in `scripts/crate_split/boundaries.toml` (MEDULLA-CONTRACT-R056.11); it may depend only
//! on crates of a lower level. Budget: 25,000 lines of Rust. Specs that land here: none yet.
//! Route new rows with `scripts/crate_split/route_rows.py`.

#![allow(dead_code)] // moved helper code, as the facade crate did
#![forbid(unsafe_code)]
#![deny(missing_docs)]

#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod plan_scope;
/// Arbitrary user-defined relational tables (CONCEPT:MEDULLA-KG.query.register-user-tables-alongside) — the durable redb
/// table store + DataFusion materialization. Behind `sql` (needs Arrow/DataFusion/redb).
#[cfg(feature = "sql")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod tables;
