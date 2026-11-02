//! medulla-finance-quant: Finance quantitative math: pricing, derivatives, state-space filters, regimes, signal models, factors, optimizer, risk, leverage, execution kernels and sizing.
//!
//! Layer L4a (MEDULLA-CONTRACT-R056.11); it may depend only
//! on crates of a lower level. Specs that land here: finance-primitives (quant kernels).

#![forbid(unsafe_code)]
#![deny(missing_docs)]

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
