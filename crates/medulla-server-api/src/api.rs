//! Seams for the crate split (MEDULLA-CONTRACT-R056.8).
//!
//! Owned by `medulla-server-api`. Each submodule holds the items moved down from a higher crate
//! and the traits through which lower crates reach behaviour that stays above them. The concrete
//! implementations are installed into [`registry::SeamRegistry`] by the server at startup.

pub mod dispatch_support;
pub mod persistence;
pub mod reentry;
pub mod registry;
