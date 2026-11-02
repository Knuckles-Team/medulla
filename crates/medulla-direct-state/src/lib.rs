//! medulla-direct-state: Direct state: the storage owner's in-place state tables and their codecs.
//!
//! Layer L2a (MEDULLA-CONTRACT-R056.11); it may depend only
//! on crates of a lower level. Budget: 25,000 lines of Rust.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod direct_state;
