//! medulla-persistence-ops: Backup, cold offload, online reshard, shard migration and store upgrade.
//!
//! Level F6 in `scripts/crate_split/boundaries.toml` (MEDULLA-CONTRACT-R056.11); it may depend only
//! on crates of a lower level. Budget: 25,000 lines of Rust. Specs that land here: durable-graph-kernel (operations).
//! Route new rows with `scripts/crate_split/route_rows.py`.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[cfg(all(feature = "cold-tier", feature = "redb"))]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod cold_tier_impl;
// Operator command over the storage kernel's registered offline store-format
// upgrades (inspect / apply), and the startup refusal that names it.
#[cfg(feature = "redb")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod store_upgrade;
// Background node-payload scrub (EH-384, CONCEPT:MEDULLA-KG.storage.node-payload-scrub):
// bounded, resumable, read-only detection of unreadable node rows between restarts.
#[cfg(feature = "redb")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod storage_scrub;

#[cfg(test)]
mod online_reshard_graft_tests;
