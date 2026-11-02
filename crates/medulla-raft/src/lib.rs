//! medulla-raft: consensus and replication. Split out of the `medulla-engine` facade (MEDULLA-CONTRACT-R056.11);
//! level 4 in `scripts/crate_split/manifest.toml`.

#![allow(dead_code)] // moved helper code, as the facade crate did
#![allow(unused_imports)] // glob imports carried over from the facade; the visibility pass prunes them
#![forbid(unsafe_code)]
#![deny(missing_docs)]

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[cfg(feature = "raft")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod raft;
