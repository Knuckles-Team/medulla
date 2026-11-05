//! medulla-finance: Finance kernels, backtests, risk, attribution, banking and execution signals.
//!
//! Layer L4 (MEDULLA-CONTRACT-R056.11); it may depend only
//! on crates of a lower level. Budget: 25,000 lines of Rust. Specs that land here: finance-primitives.

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

// Strategy research record kernels (MEDULLA-FINANCE-PRIMITIVES-R751.751).
pub mod research;

#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod attribution;
// Banking risk calculations (MEDULLA-FINANCE-PRIMITIVES-R901) and position sizing and
// distribution-shift detection (MEDULLA-FINANCE-PRIMITIVES-R900).
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod banking;
