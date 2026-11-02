//! medulla-h-mining: Mining handlers.
//!
//! Layer F9 (MEDULLA-CONTRACT-R056.11); it may depend only
//! on crates of a lower level. Budget: 25,000 lines of Rust. Specs that land here: none yet.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[cfg(feature = "mining")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod mining;
