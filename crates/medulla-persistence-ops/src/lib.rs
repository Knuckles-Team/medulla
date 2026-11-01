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
// Cluster node-info store (CONCEPT:MEDULLA-KG.sharding.cluster-topology, ADR-1 / W1.1): durable
// node_id -> {raft_addr, advertised_client_addr, tls_server_name} map that backs
// `Method::ClusterMembers`/`PlacementRoute.endpoints`, replacing the static
// `GRAPH_RAFT_GROUP_ENDPOINTS` client map. Mirrors `tenant_catalog`'s own-file,
// in-memory-cache shape. Redb-only, like its M3 siblings above.
#[cfg(feature = "redb")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod node_info_store;
// VIZ-1: durable cache for the computed hierarchical-Leiden cluster tree
// (CONCEPT:MEDULLA-KG.compute.leiden-hierarchy) one graph's `Method::ClusterHierarchyRefresh`
// produces. Own-file, own-table, keyed by graph name -- mirrors `node_info_store`'s
// shape (own redb file under the same `persist_dir`) but carries NO replication
// story (unlike node_info's Raft self-report): this is a plain per-node cache of a
// value that is always re-derivable from the live graph, never authoritative,
// never a KG node/edge write (see the `Method::ClusterHierarchyRefresh` doc for
// why membership is deliberately NOT represented as graph edges). Redb-only, like
// every other own-file side store in this module.
#[cfg(feature = "redb")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod cluster_hierarchy_store;
// Background node-payload scrub (EH-384, CONCEPT:MEDULLA-KG.storage.node-payload-scrub):
// bounded, resumable, read-only detection of unreadable node rows between restarts.
#[cfg(feature = "redb")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod storage_scrub;
