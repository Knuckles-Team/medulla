//! medulla-decision-stats: Statistical ranking for the decision ladder: fitted heads, calibration, conformal bounds, off-policy evaluation and scores.
//!
//! Layer L3 (MEDULLA-CONTRACT-R056.11); it may depend only
//! on crates of a lower level. Budget: 25,000 lines of Rust. Specs that land here: decision-engine (statistical ranking).

#![forbid(unsafe_code)]
#![deny(missing_docs)]

#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod conformal;
#[cfg(feature = "decision")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod decision;
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod ope;
