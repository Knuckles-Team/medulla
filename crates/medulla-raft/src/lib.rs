//! medulla-raft: consensus and replication. Split out of the `medulla-engine` facade (MEDULLA-CONTRACT-R056.11);
//! level 4 in `scripts/crate_split/manifest.toml`.

#![allow(clippy::too_many_arguments, clippy::items_after_test_module, clippy::new_without_default)] // signatures and fixtures carried over from the facade
#![allow(private_interfaces, private_bounds)] // visibility is narrowed by the unreachable_pub pass
#![allow(dead_code)] // moved helper code, as the facade crate did
#![allow(unused_imports)] // glob imports carried over from the facade; the visibility pass prunes them
#![forbid(unsafe_code)]
#![deny(missing_docs)]

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[cfg(feature = "raft")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod raft;
