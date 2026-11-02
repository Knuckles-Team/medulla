//! medulla-persistence: durable persistence backends. Split out of the `medulla-engine` facade (MEDULLA-CONTRACT-R056.11);
//! level 3 in `scripts/crate_split/manifest.toml`.

#![allow(clippy::too_many_arguments, clippy::items_after_test_module, clippy::new_without_default)] // signatures and fixtures carried over from the facade
#![allow(private_interfaces, private_bounds)] // visibility is narrowed by the unreachable_pub pass
#![allow(dead_code)] // moved helper code, as the facade crate did
#![allow(unused_imports)] // glob imports carried over from the facade; the visibility pass prunes them
#![forbid(unsafe_code)]
#![deny(missing_docs)]














/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[cfg(feature = "cost")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod cost;

// ---- merged from the moved module root (src/server/persistence/mod.rs) ----
// Authoritative durable graph-store contract (CONCEPT:MEDULLA-KG.storage.kg-kg).
//
// The served engine has one implementation, [`redb_backend::RedbBackend`]. The
// trait keeps mutation, recovery, read-through, backup, and Raft consumers on one
// contract without exposing storage internals. Every public mutation is committed
// before acknowledgement; bounded queues apply backpressure and never drop work.

use medulla_protocol_types::mutation_batch::MutationBatch;
use medulla_protocol_types::protocol::Method;

#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod read_through;

/// Bounded waits on the shard-owner writer thread's command replies.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod writer_reply;

// The canonical registry of every durable redb store the engine opens under a persist
// dir, plus each store's backup scope (bundled / deliberately excluded, with a reason).
// Consumed by `backup` so a bundle is self-describing about what a restore restores.
#[cfg(feature = "redb")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod durable_stores;


#[cfg(feature = "redb")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod redb_backend;

/// Recovers the concrete [`redb_backend::RedbBackend`] from a
/// `dyn PersistenceBackend` (the former `PersistenceBackend::as_redb`).
#[cfg(feature = "redb")]
pub trait AsRedb {
    /// The concrete redb backend, or `None` for any other backend.
    fn as_redb(&self) -> Option<&redb_backend::RedbBackend>;
}

#[cfg(feature = "redb")]
impl<'a> AsRedb for dyn medulla_server_api::api::persistence::PersistenceBackend + 'a {
    fn as_redb(&self) -> Option<&redb_backend::RedbBackend> {
        self.as_any()
            .and_then(|any| any.downcast_ref::<redb_backend::RedbBackend>())
    }
}

// M3 — catalog-driven resharding (CONCEPT:MEDULLA-KG.sharding.atomic-shard-swap / MEDULLA-031). Both are redb-only:
//   * `shard_migrate` — OFFLINE K-shard migration tool that rewrites an existing
//     canonical `graph-<n>.redb` set into a new K using the SAME routing,
//     preserving every durable row verbatim (incl. the tamper-evident audit chain).
//   * `tenant_catalog` — durable graph/tenant→shard (and future →node) map + a
//     read-only routing-override seam, defaulting to MEDULLA-026 FNV-1a when empty.
#[cfg(feature = "redb")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod shard_migrate;


#[cfg(feature = "redb")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod tenant_catalog;



// M3 keystone (CONCEPT:MEDULLA-KG.backend.catalog-shard-resolve / MEDULLA-034). Both redb-only:
//   * `online_reshard` — move ONE graph between shards while the engine RUNS (verbatim
//     row copy + catalog route flip + source GC), building on MEDULLA-030's copy + MEDULLA-031's
//     catalog. The keystone the offline MEDULLA-030 tool skips.
//   * `cold_offload` — time-windowed whole-graph offload of idle tenants (hibernate +
//     read-through serve) to bound RAM across many tenants.
#[cfg(feature = "redb")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod online_reshard;

#[cfg(feature = "redb")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod cold_offload;

// Provenance anchoring (CONCEPT:MEDULLA-KG.sharding.row-level-security): the periodic sweep that Merkle-anchors
// each resident graph's `:ToolCall`/`:RunTrace` provenance-node window into the
// SAME tamper-evident audit chain `security` already maintains. Redb-only (the
// audit chain and its `PROVENANCE_ANCHOR_MEMBERS` side table are redb tables).
#[cfg(feature = "security")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod provenance_anchor;

// M3 R3 — rebalancing planner (CONCEPT:MEDULLA-KG.sharding.even-load-rebalance). A PURE, deterministic policy layer
// over observable per-shard/per-graph load + the MEDULLA-031 catalog that EMITS a plan of
// `{graph, from_shard, to_shard}` moves to even out load. It does NOT execute the
// plan — that is R1 online resharding (online_reshard above). Parallel-safe, no M2 dep.
#[cfg(feature = "redb")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod rebalance;


// MEDULLA-090 — online consistent backup/restore + PITR foundation. Redb-only:
//   * `backup` — per-shard `begin_read()` MVCC snapshot (MEDULLA-027) streamed verbatim
//     (reusing MEDULLA-030's raw-row copy) into a portable bundle + manifest, ONLINE (no
//     stop-the-world), with stable admin/cross-shard recovery-boundary fingerprints;
//     preserves at-rest ciphertext + the KG-2.231 audit chain byte-for-byte.
//   * `restore_bundle` — rebuilds a persist-dir from a bundle (verbatim import via the
//     MEDULLA-030 migration engine; supports re-shard-on-restore). Backing the DR / PITR story.
#[cfg(feature = "redb")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod backup;

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
