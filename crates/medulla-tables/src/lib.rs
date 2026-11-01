//! medulla-tables: Table model: schema, property graph tables and ANN authority (everything but the table store).
//!
//! Level L4 in `scripts/crate_split/boundaries.toml` (MEDULLA-CONTRACT-R056.11); it may depend only
//! on crates of a lower level. Budget: 25,000 lines of Rust. Specs that land here: none yet.
//! Route new rows with `scripts/crate_split/route_rows.py`.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

#[cfg(feature = "sql")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod schema;
#[cfg(feature = "sql")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod index;
#[cfg(feature = "sql")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod migration;
