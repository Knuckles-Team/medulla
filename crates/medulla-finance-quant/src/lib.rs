//! medulla-finance-quant: Finance quantitative math: pricing, derivatives, state-space filters, regimes, signal models, factors, optimizer, risk, leverage, execution kernels and sizing.
//!
//! Level L4a in `scripts/crate_split/boundaries.toml` (MEDULLA-CONTRACT-R056.11); it may depend only
//! on crates of a lower level. Specs that land here: finance-primitives (quant kernels).
//! Route new rows with `scripts/crate_split/route_rows.py`.

#![allow(dead_code)] // moved helper code, as the facade crate did
#![forbid(unsafe_code)]
#![deny(missing_docs)]
// Numeric linear-algebra/optimization code reads more clearly with explicit index loops.
#![allow(clippy::needless_range_loop)]

#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod derivatives;
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod factor;
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod leverage;
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod optimizer;
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod quant;
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod regime;
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod risk;
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod signal_models;
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod statespace;

#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod execution;
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod sizing;
