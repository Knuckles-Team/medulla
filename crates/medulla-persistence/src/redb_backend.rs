//! redb write-through persistence backend (CONCEPT:MEDULLA-KG.storage.kg-kg, feature `redb`).
//!
//! The authoritative tier commits every graph mutation into an embedded
//! canonical `redb` shard database (`{persist_dir}/graph-<n>.redb`), keyed by a
//! `(graph, …)` prefix so
//! all tenants share the same tables (not a file per graph). The in-memory graph is
//! a bounded resident projection with durability-gated read-through eviction.
//!
//! ## The #1 risk: never one commit per mutation
//!
//! A shard commit is a B-tree + WAL + fsync — orders of magnitude more expensive
//! than a row write, so one per graph mutation would collapse write p99. A
//! dedicated OS thread therefore owns the file's
//! [`Shard`](crate::redb_store::shard::Shard), drains a bounded channel, and
//! folds MANY mutations into ONE admitted scope group per group-commit boundary.
//!
//! ## Every shard commit is `Durability::Immediate`, and there is no runtime knob
//!
//! The level is not this backend's to choose any more: `medulla_storage`'s
//! `physical::root::WRITE_DURABILITY` is a CONSTANT `Durability::Immediate` on
//! the one `begin_write` every kernel mutation takes, because a weaker level
//! would let redb roll a committed ledger back on crash — un-consuming an
//! acknowledged replay nonce and re-enabling the double apply. Group commit still
//! folds N ops into ONE Immediate fsync, while every batch pays that real fsync at
//! the group-commit boundary.
//!
//! Backpressure is bounded and lossless: when the writer queue is full, producers
//! wait for capacity rather than shedding persistence work. Authoritative batch
//! callers enqueue from Tokio's blocking pool and await durable completion.
//!
//! ## Tables (all keyed by graph prefix)
//!   * `nodes`          `(graph, id)            -> node properties msgpack`
//!   * `edges`          `(graph, src, tgt, ord) -> edge properties msgpack`
//!   * `ledger`         `(graph, seq)           -> ledger line`
//!   * `semantic_store` `graph                  -> semantic store blob (msgpack)`
//!   * `graph_meta`     `graph                  -> {name, graph_type} blob` (replaces manifest.json)

#[cfg(feature = "redb")]
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use redb::{ReadableTable, TableDefinition};
use tokio::sync::RwLock;

use medulla_graph_core::graph::GraphCore;
use medulla_store_apply::redb_store::store_control::GraphDump;

#[cfg(test)]
use medulla_protocol_types::mutation_batch::*;
use medulla_protocol_types::protocol::*;
#[cfg(test)]
use medulla_server_api::state::ServerState;
#[cfg(test)]
use medulla_store::redb_layout::shard_filename;
#[cfg(test)]
use tokio::sync::oneshot;

#[cfg(test)]
use medulla_server_api::api::persistence::PersistenceBackend;

// The graph table layout + the PURE durable-row machinery (Method→rows apply,
// group-commit, checkpoint/load) now live in the server-INDEPENDENT
// `crate::redb_store` (CONCEPT:MEDULLA-KG.backend.engine-modes) so the embedded API can drive the SAME
// durable format with no Tokio. This backend reuses them verbatim — ONE format,
// never duplicated — and adds only the off-reactor group-commit writer thread +
// the `PersistenceBackend` async trait wiring on top.
use medulla_store::redb_store::shard::Shard;

#[cfg(test)]
use medulla_transaction::OutboxClaimBudget;

#[cfg(all(any(test, feature = "test-support"), feature = "security"))]
pub mod audit_append_tests;
mod backend_audit;
mod backend_backup;
mod backend_load;
mod backend_open;
mod backend_routing;
mod backend_shard_routes;
pub mod commands;
#[cfg(any(feature = "compute-dist", feature = "matview"))]
mod matview_api;
#[cfg(feature = "raft")]
mod raft_api;
#[cfg(all(test, feature = "raft"))]
mod raft_linger_tests;
mod scrub_api;
#[cfg(test)]
mod scrub_api_tests;
pub mod shard_writer;
#[cfg(feature = "tsdb")]
pub mod timeseries;
mod trait_capabilities;
mod trait_envelopes;
mod trait_graph;
mod trait_graph_reads;
mod trait_impl;
mod trait_mutations;
mod trait_native;
mod trait_outbox;
mod work_item_read;
mod writer_commands;
pub mod writer_thread;
#[cfg(feature = "raft")]
mod xshard_api;

pub use commands::{
    ChangeEnvelopePayload, ChangeEnvelopesPayload, Cmd, CrossModalBatchPayload, CrossModalPayload,
    MutationBatchPayload, PageQuery,
};
pub use shard_writer::shard_index;
use shard_writer::{join_blocking_in_order, resolve_flush_threshold, resolve_shard_count};
use shard_writer::{PreparedShard, ShardWriter};
#[cfg(feature = "tsdb")]
pub use timeseries::TsReconcileReport;
#[cfg(feature = "security")]
use writer_thread::in_control_write;
use writer_thread::run;
/// `(first, last)` present Raft log index for a group, or an error (CONCEPT:MEDULLA-KG.storage.one-fsync-covers-raft).
type LogBoundsResult = Result<(Option<u64>, Option<u64>), String>;

/// Per-ATTEMPT identity for one shard write this backend admits.
///
/// The kernel resolves a batch id that already carries a durable receipt to
/// `Begin::Replay` and SKIPS it, so a reused id would silently drop a drain after
/// a restart. Hence `(pid, one nonce per process, counter)` — never derived from
/// `(raft_group, index)` or any caller key; see `redb_store::shard::drain_batch`.
fn shard_write_attempt_id(label: &str) -> String {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    static NONCE: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    let nonce = *NONCE.get_or_init(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|since| since.as_nanos() as u64)
            .unwrap_or(0)
    });
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    format!("{label}/{}-{nonce}:{seq}", std::process::id())
}
const MAX_DURABLE_SEMANTIC_BYTES: usize = 384 * 1024 * 1024;
const MAX_DURABLE_SEMANTIC_ITEMS: usize = 4_000_000;

fn decode_durable_semantic(
    bytes: &[u8],
) -> Result<medulla_semantic_index::semantic::SemanticStore, String> {
    medulla_types::msgpack::decode_bounded(
        bytes,
        medulla_types::msgpack::MsgpackLimits::new(
            MAX_DURABLE_SEMANTIC_BYTES,
            MAX_DURABLE_SEMANTIC_ITEMS,
            64,
        ),
    )
    .map_err(|_| "stored semantic index is invalid".to_string())
}
// Per-group Raft metadata (vote, applied-state pointers, last-purged), keyed by
// `(group_id, key)`. Lives in the authoritative shard alongside the log; Raft-only, so it
// stays here with the Raft helpers rather than in the shared graph store.
pub const RAFT_META: TableDefinition<(u64, &str), &[u8]> = TableDefinition::new("raft_meta");

/// Encryption-at-rest key-mismatch canary (GOC-16 / BUG-248). The table carries one
/// AEAD-sealed canary row plus one non-secret, versioned key-binding row. The sealed
/// plaintext includes the binding, so changing the plaintext metadata alone cannot
/// make a different key reference appear valid. `ShardWriter::open` verifies both
/// before spawning a writer thread or binding a listener.
///
/// The table is copied verbatim by online backup, offline shard migration, and
/// restore. A backup therefore preserves the key identity/version boundary; restore
/// never silently re-establishes a canary under whatever key happens to be present.
/// A changed key reference is an explicit rotation boundary and fails closed until
/// the documented offline re-encryption ceremony has completed.
pub const ENCRYPTION_CANARY: TableDefinition<&str, &[u8]> =
    TableDefinition::new("encryption_canary");

#[cfg(feature = "security")]
const ENCRYPTION_CANARY_KEY: &str = "v1";
#[cfg(feature = "security")]
pub const ENCRYPTION_KEY_BINDING_KEY: &str = "key-binding-v1";
#[cfg(feature = "security")]
const ENCRYPTION_CANARY_PLAINTEXT: &[u8] = b"medulla-encryption-canary";

/// What [`verify_or_establish_encryption_canary`] did, so the open path can log the
/// TRUTH rather than one undifferentiated "ENABLED" line (BUG-PE-055).
#[cfg(feature = "security")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CanaryOutcome {
    /// The store's existing canary decrypted under the configured key.
    Verified,
    /// A pre-NE-028 fixed canary was verified and upgraded to the bound pair.
    UpgradedLegacy,
    /// FIRST use of a key on an EMPTY store — a new canary was written. On a store
    /// that already held rows this is refused, not logged.
    Established,
}

/// Does this shard already hold durable rows (BUG-PE-055)?
///
/// The question the canary path could not previously ask. Establishing a canary is
/// only safe on an EMPTY store: on a populated PLAINTEXT store it silently converts
/// every subsequent write to sealed framing while every pre-existing value stays
/// plaintext, so each of those reads then fails closed with
/// `"encrypted durable value is missing sealed framing"`. `crypto`'s
/// [`TXN_RECOVERY_KEY_ENV`](medulla_kernel::crypto::TXN_RECOVERY_KEY_ENV) doc already names that
/// as "a destructive-read operation on a populated plaintext store, not a config
/// toggle"; this is the probe that lets the open path refuse it.
/// Asked of the file's CATALOG, which is the only reader class that exists before
/// any graph scope is bound — and this runs at open, before one is. That is not a
/// weakening: `graph_meta` is file-wide, and every graph that receives a row gets
/// its catalog entry backfilled in the SAME admitted group as those rows
/// (`redb_store::backfill_graph_meta_row`), so a shard with rows always has a
/// catalog entry. The one direction this can be wrong — a registered graph that
/// never received a row — refuses to establish a canary on a store that is empty
/// in fact, which is the safe direction to be wrong in.
#[cfg(feature = "security")]
fn shard_holds_durable_rows(shard: &Shard) -> Result<bool, String> {
    let control = shard.control_read()?;
    let catalog = control.open_owner_table(medulla_store::redb_store::GRAPH_META)?;
    let has_rows = {
        let mut rows = catalog.iter().map_err(|e| e.to_string())?;
        match rows.next() {
            None => false,
            Some(row) => {
                row.map_err(|e| e.to_string())?;
                true
            }
        }
    };
    Ok(has_rows)
}

/// Does this shard carry encryption-at-rest metadata (BUG-PE-055)?
///
/// Answered on the NO-KEY path, which previously never looked: a store whose values
/// are sealed opened cleanly with no key at all and then failed per-read. Startup is
/// where that belongs.
#[cfg(feature = "security")]
fn shard_carries_encryption_metadata(shard: &Shard) -> Result<bool, String> {
    let control = shard.control_read()?;
    let table = control.open_owner_table(ENCRYPTION_CANARY)?;
    for key in [ENCRYPTION_CANARY_KEY, ENCRYPTION_KEY_BINDING_KEY] {
        if table.get(key).map_err(|e| e.to_string())?.is_some() {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Verify (or, on first use, establish) that `cipher` is the SAME key and stable
/// identity/version that sealed this shard's previous data. Called from
/// `ShardWriter::open` before any writer thread spawns or any listener binds.
///
/// A pre-key-lifecycle store may have only the original fixed plaintext canary. That
/// legacy row is verified and upgraded atomically with the new binding record. A
/// store with a binding row but no canary (or vice versa) is treated as tampered and
/// fails closed; neither half is ever silently recreated.
#[cfg(feature = "security")]
fn plan_encryption_canary(
    shard: &Shard,
    cipher: &medulla_kernel::crypto::ValueCipher,
) -> Result<CanaryPlan, String> {
    let (existing_canary, existing_binding) = {
        let control = shard.control_read()?;
        let table = control.open_owner_table(ENCRYPTION_CANARY)?;
        let canary = table
            .get(ENCRYPTION_CANARY_KEY)
            .map_err(|e| e.to_string())?
            .map(|v| v.value().to_vec());
        let binding = table
            .get(ENCRYPTION_KEY_BINDING_KEY)
            .map_err(|e| e.to_string())?
            .map(|v| v.value().to_vec());
        (canary, binding)
    };

    let configured_ref = cipher.key_ref();
    let expected_plaintext = medulla_kernel::crypto::encryption_canary_plaintext(configured_ref);
    match (existing_canary, existing_binding) {
        (Some(sealed), Some(binding_bytes)) => {
            let binding = medulla_kernel::crypto::EncryptionKeyBinding::decode(&binding_bytes)?;
            let persisted_ref = binding.key_ref()?;
            if &persisted_ref != configured_ref {
                return Err(format!(
                    "refusing to open the durable graph store: configured encryption key \
                     reference {}@{} does not match persisted reference {}@{}; complete \
                     the documented offline key-rotation procedure before changing it",
                    configured_ref.id,
                    configured_ref.version,
                    persisted_ref.id,
                    persisted_ref.version,
                ));
            }
            let plaintext = cipher.unseal(&sealed).map_err(|_| {
                format!(
                    "refusing to open the durable graph store: {} does not match the key \
                     that previously encrypted this store (canary decryption failed)",
                    medulla_kernel::crypto::ENCRYPTION_KEY_ENV,
                )
            })?;
            if plaintext != expected_plaintext {
                return Err(
                    "refusing to open the durable graph store: encryption canary binding \
                     is invalid or tampered"
                        .to_string(),
                );
            }
            Ok(CanaryPlan::Verified(CanaryOutcome::Verified))
        }
        (Some(sealed), None) => {
            // Upgrade the pre-NE-028 fixed canary only after proving the configured
            // key can decrypt it. The binding is then written in the SAME transaction
            // as the new authenticated canary, so a crash leaves either the legacy
            // row (safe to retry) or the complete new pair.
            let plaintext = cipher.unseal(&sealed).map_err(|_| {
                format!(
                    "refusing to open the durable graph store: {} does not match the key \
                     that previously encrypted this store (legacy canary decryption failed)",
                    medulla_kernel::crypto::ENCRYPTION_KEY_ENV,
                )
            })?;
            if plaintext != ENCRYPTION_CANARY_PLAINTEXT {
                return Err(
                    "refusing to open the durable graph store: legacy encryption canary is \
                     invalid or tampered"
                        .to_string(),
                );
            }
            let binding =
                medulla_kernel::crypto::EncryptionKeyBinding::from_key_ref(configured_ref);
            let binding_bytes = binding.encode()?;
            let sealed = cipher.seal(&expected_plaintext);
            Ok(CanaryPlan::Write {
                sealed,
                binding_bytes,
                outcome: CanaryOutcome::UpgradedLegacy,
            })
        }
        (None, Some(_)) => Err(
            "refusing to open the durable graph store: encryption key binding exists but \
             its canary is missing"
                .to_string(),
        ),
        (None, None) => {
            // BUG-PE-055: this arm used to be taken IDENTICALLY for a brand-new store
            // and for a populated store that was written in plaintext — it established
            // a canary and started encrypting, silently, logging only
            // "redb encryption-at-rest ENABLED". Every pre-existing value then became
            // unreadable one read at a time. Establishing a canary is only ever
            // correct on an EMPTY store.
            if shard_holds_durable_rows(shard)? {
                return Err(format!(
                    "refusing to open the durable graph store: {} is set but this store \
                     carries no encryption canary and already holds durable rows, so it \
                     was written in PLAINTEXT. Enabling encryption-at-rest on a populated \
                     plaintext store is a destructive-read operation, not a config \
                     toggle: every existing value would fail to unseal. Complete the \
                     documented offline re-encryption procedure into a fresh persist \
                     dir, or unset {} to keep serving this store as it is.",
                    medulla_kernel::crypto::ENCRYPTION_KEY_ENV,
                    medulla_kernel::crypto::ENCRYPTION_KEY_ENV,
                ));
            }
            let binding =
                medulla_kernel::crypto::EncryptionKeyBinding::from_key_ref(configured_ref);
            let binding_bytes = binding.encode()?;
            let sealed = cipher.seal(&expected_plaintext);
            Ok(CanaryPlan::Write {
                sealed,
                binding_bytes,
                outcome: CanaryOutcome::Established,
            })
        }
    }
}

/// What [`plan_encryption_canary`] decided this store needs, WITHOUT doing it.
///
/// The two halves are separate because the decision is per-STORE while the
/// refusal is per-PERSIST-DIR. A dir holds K shard files, each with its own
/// canary table and its own local view of "am I empty?", and `open` refuses the
/// whole dir if ANY of them refuses. While establishing the canary happened
/// inside the per-shard decision, a refusal on the one shard that held the data
/// still left every OTHER shard — genuinely empty, and so genuinely eligible to
/// have a canary established — durably converted to a key-required store.
///
/// The dir was then bricked in both directions: it would not open WITH the key
/// (the data shard refuses "this store was written in PLAINTEXT") and would not
/// open WITHOUT it (the converted shards refuse "this store carries
/// encryption-at-rest metadata"), which makes the first refusal's own documented
/// remediation -- "unset MEDULLA_ENCRYPTION_KEY to keep serving this
/// store as it is" -- false. K is greater than one by default in a served
/// process (`resolve_shard_count` autosizes from CPUs, and under Raft K == N
/// groups), so this was the ordinary case, not an edge one.
///
/// Planning first makes the refusal byte-clean: the open path plans every shard,
/// and only once every shard has agreed does it apply any plan.
#[cfg(feature = "security")]
enum CanaryPlan {
    /// This store already carries a canary this key verifies, or carries none
    /// and needs none. Nothing to write.
    Verified(CanaryOutcome),
    /// This store needs its canary written before it can be served.
    Write {
        sealed: Vec<u8>,
        binding_bytes: Vec<u8>,
        outcome: CanaryOutcome,
    },
}

/// Commit a [`CanaryPlan`], after every store in the persist dir has agreed.
#[cfg(feature = "security")]
fn apply_encryption_canary(shard: &Shard, plan: CanaryPlan) -> Result<CanaryOutcome, String> {
    match plan {
        CanaryPlan::Verified(outcome) => Ok(outcome),
        CanaryPlan::Write {
            sealed,
            binding_bytes,
            outcome,
        } => {
            write_encryption_canary(shard, &sealed, &binding_bytes)?;
            Ok(outcome)
        }
    }
}

/// Write the sealed canary and its key-binding row as ONE control-only commit, so
/// a crash leaves the store's previous state or the complete new pair, never half.
/// Control-only because `encryption_canary` is file-wide: it belongs to the file,
/// not to any graph it hosts.
#[cfg(feature = "security")]
fn write_encryption_canary(
    shard: &Shard,
    sealed: &[u8],
    binding_bytes: &[u8],
) -> Result<(), String> {
    in_control_write(shard, "encryption_canary", |write| {
        let mut table = write.control().open_table(ENCRYPTION_CANARY)?;
        table
            .insert(ENCRYPTION_CANARY_KEY, sealed)
            .map_err(|e| e.to_string())?;
        table
            .insert(ENCRYPTION_KEY_BINDING_KEY, binding_bytes)
            .map(|_| ())
            .map_err(|e| e.to_string())
    })
}

/// Adaptive group-commit micro-linger tuning for the redb writer (CONCEPT:MEDULLA-KG.backend.adaptive-linger-coalesce).
///
/// Live profiling of the `medulla-redb-writer` thread showed it pinned ~100% on ext4
/// writeback (disk ~83% util, ~50ms write latency, queue depth 42) while every
/// tokio worker sat idle — it is the ingestion write ceiling. The machinery already
/// group-commits (many `Cmd::Mutation` fold into ONE `WriteTransaction`/fsync), but
/// because every authoritative write carries a commit-before-ack `done` oneshot,
/// `Pending::has_barrier()` is ALWAYS true, so the writer commits the instant the
/// channel momentarily drains. With low in-flight write concurrency (serial awaits —
/// the idle workers) the batch is whatever incidentally sat in the channel, i.e.
/// ~1 op ⇒ ~1 fsync per write. The batching machinery was starved of a window.
///
/// This adds a bounded, adaptive linger: when about to commit a SHALLOW barrier
/// batch, spend ONE `recv_timeout(linger)` letting more concurrent writers arrive,
/// then drain again. "Shallow" counts both graph mutations and Raft log entries:
/// both are rows in the same pending durable transaction, so excluding a raft-only
/// append would restore the pathological one-append/one-fsync shape EH-290 measured.
/// It MIRRORS the in-memory write-coalescer's `max_linger`
/// (CONCEPT:MEDULLA-KG.sharding.per-graph-write-coalescer, `write_coalescer.rs`) but for the DURABLE tier — it does NOT
/// touch the coalescer. Durability is unchanged: authoritative writes still commit
/// `Durability::Immediate` BEFORE their `done` fires; we only widen the batch, never
/// defer an ack past its commit. A crash before commit still loses only un-acked writes.
#[cfg(any(test, feature = "test-support"))]
#[derive(Debug)]
pub struct RedbGroupCommitTestControl {
    entered: std::sync::mpsc::SyncSender<()>,
    release: std::sync::Mutex<Option<std::sync::mpsc::Receiver<()>>>,
}

#[cfg(any(test, feature = "test-support"))]
impl RedbGroupCommitTestControl {
    pub fn new() -> (
        Arc<Self>,
        std::sync::mpsc::Receiver<()>,
        std::sync::mpsc::SyncSender<()>,
    ) {
        let (entered, entered_rx) = std::sync::mpsc::sync_channel(1);
        let (release, release_rx) = std::sync::mpsc::sync_channel(1);
        (
            Arc::new(Self {
                entered,
                release: std::sync::Mutex::new(Some(release_rx)),
            }),
            entered_rx,
            release,
        )
    }

    fn wait_until_released(&self) {
        let _ = self.entered.send(());
        let release = self
            .release
            .lock()
            .expect("group-commit test control lock poisoned")
            .take();
        if let Some(release) = release {
            medulla_kernel::test_rendezvous::recv_within(
                &release,
                "the group-commit test control releasing the writer",
            );
        }
    }
}

#[cfg(any(test, feature = "test-support"))]
pub struct ReleaseOnDrop(Option<std::sync::mpsc::SyncSender<()>>);

#[cfg(any(test, feature = "test-support"))]
impl ReleaseOnDrop {
    pub fn new(sender: std::sync::mpsc::SyncSender<()>) -> Self {
        Self(Some(sender))
    }

    pub fn release(&mut self) -> Result<(), std::sync::mpsc::SendError<()>> {
        self.0
            .take()
            .expect("release guard must be used at most once")
            .send(())
    }
}

#[cfg(any(test, feature = "test-support"))]
impl Drop for ReleaseOnDrop {
    fn drop(&mut self) {
        if let Some(sender) = self.0.take() {
            let _ = sender.send(());
        }
    }
}

#[derive(Debug, Clone)]
pub struct RedbGroupCommitConfig {
    /// Max time to linger for more concurrent writers before committing a shallow
    /// barrier batch. `Duration::ZERO` disables lingering entirely (commit-on-drain
    /// = today's behavior, used as the bench baseline).
    pub linger: Duration,
    /// Only linger when the pending graph-mutation + Raft-entry count is BELOW
    /// this — a deep batch already coalesces well, so lingering buys nothing and
    /// just adds latency (adaptive).
    pub shallow_threshold: usize,
    /// Test-only gate used to hold the writer at the start of the linger window
    /// while the fixture queues the rest of its burst. Production opens never set
    /// this, so the live writer path has no synchronization hook.
    #[cfg(any(test, feature = "test-support"))]
    pub test_control: Option<Arc<RedbGroupCommitTestControl>>,
}

impl RedbGroupCommitConfig {
    /// Resolve from env (Configuration discipline: read once at backend open).
    ///   * `MEDULLA_REDB_GROUP_LINGER_US` — linger microseconds (default
    ///     `1000` = 1ms; `0` disables lingering / restores commit-on-drain).
    ///   * `MEDULLA_REDB_GROUP_SHALLOW` — shallow durable-work threshold
    ///     (default `32`); the writer lingers only while the pending graph-mutation
    ///     plus Raft-entry count is under it.
    pub fn from_env() -> Self {
        let linger_us = std::env::var("MEDULLA_REDB_GROUP_LINGER_US")
            .ok()
            .and_then(|s| s.trim().parse::<u64>().ok())
            .unwrap_or(1000);
        let shallow = std::env::var("MEDULLA_REDB_GROUP_SHALLOW")
            .ok()
            .and_then(|s| s.trim().parse::<usize>().ok())
            .unwrap_or(32);
        Self {
            linger: Duration::from_micros(linger_us),
            // Never above the 4096 early-flush bound; at least 1.
            shallow_threshold: shallow.clamp(1, 4096),
            #[cfg(any(test, feature = "test-support"))]
            test_control: None,
        }
    }
}

impl Default for RedbGroupCommitConfig {
    fn default() -> Self {
        Self::from_env()
    }
}

/// Group-commit observability for the redb writer (CONCEPT:MEDULLA-KG.backend.adaptive-linger-coalesce), mirroring
/// `write_coalescer::BatchStats`. `ops / commits` is the average batch size = the
/// fsyncs-saved ratio; `lingered` counts commits that paid a micro-linger window.
/// `linger_waiting` exposes whether the writer is currently inside that bounded
/// receive window, which makes operational probes and deterministic contention
/// tests observe the mechanism instead of guessing from scheduler timing.
#[derive(Debug, Default)]
pub struct RedbCommitStats {
    /// Group-commit `WriteTransaction`s issued on the run-loop barrier/timeout path.
    pub commits: AtomicU64,
    /// Total graph ops folded across those commits.
    pub ops: AtomicU64,
    /// How many of those commits paid a micro-linger window.
    pub lingered: AtomicU64,
    /// True only while the writer is blocked in the bounded micro-linger receive.
    linger_waiting: AtomicBool,
}

impl RedbCommitStats {
    fn record(&self, ops: usize, lingered: bool) {
        self.commits.fetch_add(1, Ordering::Relaxed);
        self.ops.fetch_add(ops as u64, Ordering::Relaxed);
        if lingered {
            self.lingered.fetch_add(1, Ordering::Relaxed);
        }
    }
    pub fn commits(&self) -> u64 {
        self.commits.load(Ordering::Relaxed)
    }
    pub fn ops(&self) -> u64 {
        self.ops.load(Ordering::Relaxed)
    }
    pub fn lingered(&self) -> u64 {
        self.lingered.load(Ordering::Relaxed)
    }
    /// Whether this writer is currently waiting for a concurrent mutation in its
    /// bounded micro-linger window.
    pub fn is_linger_waiting(&self) -> bool {
        self.linger_waiting.load(Ordering::Acquire)
    }
    /// Average group-commit batch size (`ops / commits`); 0.0 before any commit.
    pub fn avg_batch(&self) -> f64 {
        let c = self.commits();
        if c == 0 {
            0.0
        } else {
            self.ops() as f64 / c as f64
        }
    }
}

/// [`medulla_storage::PrivatePayloadIntegrity`] for the admin-mutations store, backed by the
/// SAME transaction-recovery-plan cipher (D-ORC-50) that sealed the bytes in the first
/// place (`medulla_h_txn::txn::seal_txn_recovery_plan`/`open_txn_recovery_plan`). The storage
/// kernel enforces it on every private-payload read/write, so without a real authority
/// here every 2PC recovery-plan write/read fails closed — not optional plumbing.
#[cfg(feature = "security")]
struct TxnRecoveryPrivateIntegrity(medulla_kernel::crypto::ValueCipher);

/// Authenticate one sealed private payload against the binding carried by its
/// parent receipt. SPARQL recovery binds the exact plaintext digest. Transaction
/// recovery binds the stable semantic intent so a retry may carry fresh OCC
/// observations without becoming a different operation.
#[cfg(feature = "security")]
pub fn authenticate_private_payload(
    cipher: &medulla_kernel::crypto::ValueCipher,
    sealed: &[u8],
    expected_binding: &str,
) -> Result<(), String> {
    use sha2::Digest;
    let plaintext = cipher.unseal(sealed)?;
    let exact_binding = hex::encode(sha2::Sha256::digest(&plaintext));
    let semantic_binding = medulla_server_api::txn::GraphTxnState::decode_recovery_plan(
        &plaintext,
        "private-recovery-integrity".to_string(),
    )
    .and_then(|txn| txn.replay_intent_digest())
    .ok();
    [Some(exact_binding), semantic_binding]
        .into_iter()
        .flatten()
        .any(|binding| binding == expected_binding)
        .then_some(())
        .ok_or_else(|| {
            "private recovery payload digest does not match its parent receipt".to_string()
        })
}

#[cfg(feature = "security")]
impl medulla_storage::PrivatePayloadIntegrity for TxnRecoveryPrivateIntegrity {
    fn authenticate(&self, sealed: &[u8], expected_plaintext_digest: &str) -> Result<(), String> {
        authenticate_private_payload(&self.0, sealed, expected_plaintext_digest)
    }
}

/// Resolve the admin-mutations store's private-payload integrity authority. Reads the
/// SAME `MEDULLA_TXN_RECOVERY_KEY` (falling back to the shared data key) as every
/// [`ShardWriter`]'s own `txn_recovery_cipher`, but as a function of the environment
/// rather than of any one shard (none is constructed yet at this call site). `None` without the
/// `security` feature or the key: the write/read path already fails closed itself there.
#[cfg(feature = "security")]
pub(crate) fn admin_mutations_private_integrity(
) -> Option<Arc<dyn medulla_storage::PrivatePayloadIntegrity>> {
    medulla_kernel::crypto::ValueCipher::from_env_for_txn_recovery().map(|cipher| {
        Arc::new(TxnRecoveryPrivateIntegrity(cipher))
            as Arc<dyn medulla_storage::PrivatePayloadIntegrity>
    })
}

#[cfg(not(feature = "security"))]
pub(crate) fn admin_mutations_private_integrity(
) -> Option<Arc<dyn medulla_storage::PrivatePayloadIntegrity>> {
    None
}

/// Operator-facing identity of the ONE physical `admin-mutations.redb` owner file.
pub(crate) const ADMIN_MUTATIONS_STORE: &str = "epistemic-graph:admin-mutations";

/// The owner domain of `admin-mutations.redb`: it declares no owner tables of its own,
/// so `medulla_storage` materializes only the ledger census for it.
pub(crate) type AdminOwner = medulla_storage::LedgerOnlyOwner;
/// One kernel-issued scoped read over that store, as the consumers name it.
pub(crate) type AdminScopedRead<'a> = medulla_storage::ScopedRead<'a, AdminOwner>;

/// The kernel-owned admin-mutations coordinator store (RF-RULING-004): the storage
/// kernel that owns the file, the one mutation kernel it issued, and the single bound
/// serving scope. It hands no database back out. Every row the file holds is the
/// ledger's own bookkeeping, so `create_owner` materializes the whole declared census
/// at creation and no "ensure the table exists" bootstrap of ours remains.
type AdminMutationStore = medulla_store::sidecar_store::SidecarStore<AdminOwner>;

/// Open (creating if absent) the one admin-mutations owner file.
fn open_admin_mutations(path: &std::path::Path) -> Result<AdminMutationStore, String> {
    AdminMutationStore::open_with(
        path,
        ADMIN_MUTATIONS_STORE,
        medulla_server_ops::api::admin::saga::cluster_admin_scope_identity()?,
        admin_mutations_private_integrity(),
        medulla_kernel::store_authority::process_authority(),
    )
}

/// Handle to the redb write-through tier (CONCEPT:MEDULLA-KG.storage.kg-kg / MEDULLA-026). The dispatch
/// path holds an `Arc` of this and calls `record`/`record_durable`; each routes by
/// graph to one of K independent single-writer [`ShardWriter`]s, so K cores commit in
/// parallel. K=1 holds exactly one shard backed by canonical `graph-0.redb`.
pub struct RedbBackend {
    /// The K shards (len >= 1). Index `shard_index(graph_fname, K)` owns a graph.
    shards: Vec<ShardWriter>,
    /// Optional tenant catalog OVERRIDE for graph→shard routing (CONCEPT:MEDULLA-KG.sharding.empty-catalog-routing, M3).
    /// `None` (the default) ⇒ pure MEDULLA-026 FNV-1a routing, byte-for-byte unchanged. When
    /// `Some` AND it holds an explicit entry for a graph, that entry's shard wins
    /// (enabling rebalanceable / resharded placement); a graph with no entry STILL
    /// falls back to FNV-1a inside `TenantCatalog::resolve_shard`. So an empty catalog
    /// is indistinguishable from no catalog — the seam never destabilizes MEDULLA-026.
    catalog: Option<Arc<crate::tenant_catalog::TenantCatalog>>,
    /// Routing quiesce barrier for online resharding (CONCEPT:MEDULLA-KG.backend.catalog-shard-resolve). Catalog-attached
    /// durable writes resolve their shard + enqueue their op while holding a SHARED READ
    /// guard; [`RedbBackend::reshard_graph`] holds the EXCLUSIVE WRITE guard across a
    /// graph's move, so the route flip can never interleave a write (no lost / misrouted
    /// rows). When NO catalog is attached (the default) the write path never touches this
    /// — MEDULLA-026 is byte-for-byte unchanged.
    routing_epoch: Arc<RwLock<()>>,
    /// Local durable projection of cluster-wide/admin saga authority: in clustered
    /// serving this file is replayable consensus state on every group member rather than
    /// a pod-local coordinator, and single-node serving uses the same image directly.
    admin_mutations: AdminMutationStore,
    /// Durable cluster-topology self-report store (CONCEPT:MEDULLA-KG.sharding.cluster-topology, ADR-1 / W1.1).
    /// Always opened (like `admin_mutations` above) so the shape of `RedbBackend`
    /// doesn't vary with whether Raft happens to be configured; it is populated
    /// only when a clustered node self-reports at startup (`raft::node::start`).
    /// See `medulla_server::persistence::node_info_store` for the replication story.
    node_info: Arc<super::node_info_store::NodeInfoStore>,
    /// Durable cluster-hierarchy cache (CONCEPT:MEDULLA-KG.compute.leiden-hierarchy, VIZ-1). Own-file,
    /// like `node_info` above, but carries no replication story — a cache entry
    /// is node-local and always safely recomputable. See
    /// `medulla_server::persistence::cluster_hierarchy_store` for why membership never
    /// rides graph nodes/edges.
    cluster_hierarchy: Arc<super::cluster_hierarchy_store::ClusterHierarchyStore>,
}

/// Rebuild a live [`GraphCore`] from a durable [`GraphDump`] (CONCEPT:MEDULLA-KG.storage.100m-tenant —
/// tenant rehydration). Uses the SAME `add_node`/`add_edge`/semantic-restore path
/// `load_into` uses, so a rehydrated graph is byte-identical to a freshly loaded one.
/// The core is cleared first so a re-rehydrate is idempotent.
pub fn rehydrate_core_from_dump(core: &GraphCore, dump: &GraphDump) -> Result<(), String> {
    // Validate the binary-reconciled core+dynamic set before clearing the live
    // projection.  Reshard/hibernate rehydration therefore has the same atomic
    // engine-upgrade rule as ordinary startup.
    #[cfg(feature = "shacl")]
    medulla_server_api::schema_compose::validate_and_compose(&dump.schema_sources)?;
    core.clear();
    core.install_schema_sources(std::sync::Arc::clone(&dump.schema_sources));
    for (id, props) in &dump.nodes {
        core.add_node(id.clone(), props.clone());
    }
    for (src, tgt, props) in &dump.edges {
        let _ = core.add_edge(src.clone(), tgt.clone(), props.clone());
    }
    if !dump.semantic.is_empty() {
        if let Ok(store) = decode_durable_semantic(&dump.semantic) {
            *core.semantic_store.write() = store;
        }
    }
    Ok(())
}
