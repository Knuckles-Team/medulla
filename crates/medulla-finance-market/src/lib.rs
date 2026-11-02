//! medulla-finance-market: Market data primitives: quotes, bars, books and their kernels.
//!
//! Layer L3 (MEDULLA-CONTRACT-R056.11); it may depend only
//! on crates of a lower level. Budget: 25,000 lines of Rust. Specs that land here: finance-primitives (market data).

#![forbid(unsafe_code)]
#![deny(missing_docs)]

#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod market;
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod signals;
