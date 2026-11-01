//! medulla-finance: Finance kernels, backtests, risk, attribution, banking and execution signals.
//!
//! Level L4 in `scripts/crate_split/boundaries.toml` (MEDULLA-CONTRACT-R056.11); it may depend only
//! on crates of a lower level. Budget: 25,000 lines of Rust. Specs that land here: finance-primitives.
//! Route new rows with `scripts/crate_split/route_rows.py`.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod exchange;
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod forensic;
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod ghostfolio_import;
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod golden_fixture;
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod lot_accounting;
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod strategy;
