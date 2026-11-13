//! medulla-tables: Table model: schema, property graph tables and ANN authority (everything but the table store).
//!
//! Layer L4 (MEDULLA-CONTRACT-R056.11); it may depend only
//! on crates of a lower level. Budget: 25,000 lines of Rust. Specs that land here: none yet.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

/// Default of the registry knob `tables.deferred_fk_pending_max` (MEDULLA-ENGINE-PARITY-R011.1):
/// the most deferred foreign keys one transaction may touch. The knob table in
/// `medulla-server-api` takes this value as its default, so there is one source;
/// a store the composition root has not installed a resolved value on uses it.
pub const DEFERRED_FK_PENDING_MAX_DEFAULT: u64 = 100_000;

/// Default of the registry knob `tables.trigger_depth_max` (MEDULLA-ENGINE-PARITY-R012.2):
/// the deepest chain of triggers one write may nest. The knob table in
/// `medulla-server-api` takes this value as its default, so there is one source.
pub const TRIGGER_DEPTH_MAX_DEFAULT: u64 = 16;

#[cfg(feature = "sql")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod index;
#[cfg(feature = "sql")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod migration;
#[cfg(feature = "sql")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod schema;
#[cfg(feature = "sql")]
pub mod sequence;
#[cfg(feature = "sql")]
pub mod trigger;
