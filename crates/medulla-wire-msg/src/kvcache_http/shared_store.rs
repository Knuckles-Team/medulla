//! Networked (mutation-store-backed) shared KV-cache backend (CONCEPT:MEDULLA-KG.backend.networked-shared-kv).
//!
//! Where [`medulla_kvcache::SharedKvIndex`] (MEDULLA-186) is an **in-process, ephemeral**
//! content-addressed index, [`SharedKvStoreBackend`] is the same
//! [`medulla_kvcache::SharedKvBackend`] seam over the engine's **durable, mutation-store-backed
//! KV store** ([`medulla_server::kv::KvStore`], the `kv.redb` the `Method::KvGet`/`KvPut`
//! wire surface owns). This is the documented follow-up the `medulla-kvcache` crate report and
//! the HTTP surface both point at ("A follow-up networked backend would swap this for an
//! RPC / object-store client implementing the SAME `SharedKvBackend` trait").
//!
//! ## Why this makes the cache FLEET-SHARED
//!
//! Every LLM-serving instance (a vLLM / LMCache worker) that reaches THIS engine — over
//! the `/kv` HTTP surface or the `KvGet`/`KvPut` wire methods — reads and writes the SAME
//! `kv.redb`. So a prefix block a warm worker (instance A) PUTs is a cache **HIT** for a
//! cold worker (instance B): cross-instance prefix reuse *through the engine*, not merely
//! within one process. And because the store is redb-durable (commit-before-ack), the
//! shared cache **survives an engine restart** — the L2 tier the `docs/interfaces/kvcache.md`
//! hierarchy calls "durable + dedup · shared cross-instance".
//!
//! ## Storage layout
//!
//! One namespace holds the blocks, one holds the invalidation epoch:
//!
//! * `kvcache-blocks / <token-hash> -> ColdFrame` — the block as the self-describing cold
//!   frame of `medulla-kvcache` (codec, original length, [`DataVersion`] it was derived at,
//!   content digest, stored bytes). This namespace is the COLD rung of the tier ladder and
//!   the one durable copy of a block; every block is written through to it before the
//!   put is acknowledged, and a block a restarted or second instance has not seen is
//!   adopted from its row ([`medulla_kvcache::TieredCache::adopt_cold`]).
//! * `kvcache-meta / data_version -> <u64 BE>` — the store's current data version
//!   (CONCEPT:MEDULLA-KG.storage.content-addressed-put). Absent ⇒ version-tracking inactive
//!   (`Agnostic`). Persisted so EVERY instance agrees on freshness across the fleet — a
//!   graph write on one node invalidates stale derived-context for all of them.
//!
//! ## Freshness is gated LAZILY (durable-appropriate)
//!
//! `SharedKvIndex::set_data_version` EAGERLY retires stale entries to free RAM. A durable
//! store gates **lazily on read** instead (a stale entry reads as absent) and persists the
//! new epoch to `kvcache-meta`; scanning-and-deleting the whole namespace on every graph
//! write would be an unbounded cost per write. Callers that want to reclaim disk run the
//! bounded [`SharedKvStoreBackend::retire_stale`] sweep out of band. A pure content-addressed
//! page (`Agnostic`) never reads the epoch at all — the dedup hot path stays a single redb
//! point-read.
//!
//! ## Metrics (Seam-6 hit-rate, W3.7 shape)
//!
//! Atomic `get_hits` / `get_misses` / `dedup_hits` / `put_new` / `stale_missed` counters
//! feed [`SharedKvStoreBackend::stats`] and a periodic `tracing` hit-rate line in the SAME
//! shape as `medulla-core`'s `result_cache` (`target: "medulla::kvcache"`, fields
//! `hits`/`misses`/`hit_rate`), plus a per-op `trace` carrying the block hash, hit/miss,
//! and the instance id (so cross-instance reuse is visible in logs).

use medulla_kernel::lock_recovery::LockRecovery;
use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use medulla_kvcache::{
    content_hash, decode_cold_frame, encode_cold_frame, AdmissionRefusal, Block, CacheEntryMeta,
    ColdFrame, DataVersion, KvTierError, PolicyTier, SharedKvBackend, StoredBlock, Tier,
    TierPolicy, TieredCache,
};

use medulla_server_blob::kv::KvStore;

use super::cold_namespace::KvNamespaceColdStore;
use super::BackendError;

/// KV namespace for the framed KV-cache block bytes.
const BLOCK_NS: &str = "kvcache-blocks";
/// KV namespace for the invalidation-epoch meta key.
const META_NS: &str = "kvcache-meta";
/// Meta key holding the store's current [`DataVersion`] (absent ⇒ `Agnostic`).
const VERSION_KEY: &str = "data_version";
/// Emit the periodic hit-rate metric once every this many accesses (mirrors
/// `result_cache::maybe_log_rate`'s 1024-access cadence — cheap, never floods).
const HIT_RATE_LOG_EVERY: u64 = 1024;

/// The tenant every shared block is ledgered under: the cache is fleet-shared, so a block
/// belongs to the deployment, not to one caller.
const SHARED_TENANT: &str = "shared";
/// The `KnobConfig` defaults of `kvcache.answer_max_bytes` (256 MiB) and
/// `kvcache.min_worth` (0.001), for a backend built without the loaded knobs.
pub const DEFAULT_BLOCK_POLICY: TierPolicy = TierPolicy {
    max_bytes: 268_435_456,
    min_worth: 0.001,
};

/// The byte budgets of the three rungs and the graph-importance weight, as the registry
/// knobs `kvcache.hot_bytes`, `kvcache.warm_bytes`, `kvcache.cold_bytes` and
/// `kvcache.importance_weight` resolve them (MEDULLA-DURABLE-KERNEL-R083.5/R083.6).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TierBudgets {
    /// HOT (raw, in RAM) byte budget.
    pub hot_bytes: u64,
    /// WARM (compressed, in RAM) byte budget.
    pub warm_bytes: u64,
    /// COLD (durable `kvcache-blocks` namespace) byte budget.
    pub cold_bytes: u64,
    /// Weight of the graph-importance term in the demotion score.
    pub importance_weight: f64,
}

/// The `KnobConfig` defaults (1 GiB, 4 GiB, 64 GiB, weight 1.0), for a backend built
/// without the loaded knobs.
pub const DEFAULT_TIER_BUDGETS: TierBudgets = TierBudgets {
    hot_bytes: 1_073_741_824,
    warm_bytes: 4_294_967_296,
    cold_bytes: 68_719_476_736,
    importance_weight: 1.0,
};

/// What [`SharedKvStoreBackend::store_block_priced`] did with a block.
#[derive(Clone, Debug, PartialEq)]
pub enum StoreOutcome {
    /// A new block was stored.
    Created,
    /// The block was already present (or its version stamp was refreshed).
    Deduplicated,
    /// The tier policy refused the block; nothing was stored. A refusal is a miss.
    Refused(AdmissionRefusal),
}

/// Occupancy + hit-rate counters for a [`SharedKvStoreBackend`], shaped like
/// [`medulla_kvcache::SharedStats`] / the `/kv/stats` JSON so a dashboard consumes either
/// backend identically (the W3.7 Seam-6 hit-rate export shape).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SharedKvStoreStats {
    /// Lifetime `get_block` hits (a fresh block served).
    pub get_hits: u64,
    /// Lifetime `get_block` misses (absent OR stale).
    pub get_misses: u64,
    /// Lifetime `put_block` calls that found the block ALREADY present — a
    /// cross-instance dedup win (another instance had already stored it).
    pub dedup_hits: u64,
    /// Lifetime `put_block` calls that created a NEW block.
    pub put_new: u64,
    /// Subset of `get_misses` that missed specifically because the entry was STALE
    /// (version-invalidated), not absent — the invalidation-proof counter.
    pub stale_missed: u64,
}

impl SharedKvStoreStats {
    /// Hit-rate over all `get_block` calls (`0.0` when there were none).
    pub fn hit_rate(&self) -> f64 {
        let total = self.get_hits + self.get_misses;
        if total == 0 {
            0.0
        } else {
            self.get_hits as f64 / total as f64
        }
    }
}

/// A content-addressed shared KV-cache backend over the engine's durable,
/// mutation-store-backed [`KvStore`] (CONCEPT:MEDULLA-KG.backend.networked-shared-kv).
///
/// The canonical NETWORKED [`SharedKvBackend`]: identical blocks are stored ONCE in
/// `kv.redb` and shared by every serving instance that reaches this engine. Holds an
/// `Arc<KvStore>` (so the HTTP surface and the `KvGet`/`KvPut` wire methods share ONE
/// store) plus atomic hit-rate counters; all real work is `&self` (redb ops are `&self`,
/// counters are atomic), so the `&mut self` in the trait is a pure signature detail.
pub struct SharedKvStoreBackend {
    store: Arc<KvStore>,
    /// The tier ladder over the blocks this instance has served or stored; its cold rung
    /// is the `kvcache-blocks` namespace of `store`, which keeps the one durable copy.
    cache: Mutex<TieredCache<String, Block>>,
    /// This serving instance's id — stamped on every metric so cross-instance reuse is
    /// visible in the logs (e.g. instance B logging a hit on a hash instance A stored).
    instance_id: String,
    /// Admission ledger of the stored blocks: bookkeeping only, the bytes live in `store`.
    policy: Mutex<PolicyTier>,
    min_worth: f64,
    get_hits: AtomicU64,
    get_misses: AtomicU64,
    dedup_hits: AtomicU64,
    put_new: AtomicU64,
    stale_missed: AtomicU64,
    accesses: AtomicU64,
}

impl SharedKvStoreBackend {
    /// Wrap a durable [`KvStore`] as a fleet-shared KV-cache backend. `instance_id`
    /// labels this serving instance in the metrics/logs.
    pub fn new(store: Arc<KvStore>, instance_id: impl Into<String>) -> Self {
        Self::with_policy(store, instance_id, DEFAULT_BLOCK_POLICY)
    }

    /// [`new`](Self::new) with the tier policy the server loaded from its knobs.
    pub fn with_policy(
        store: Arc<KvStore>,
        instance_id: impl Into<String>,
        policy: TierPolicy,
    ) -> Self {
        Self::with_tiers(store, instance_id, policy, DEFAULT_TIER_BUDGETS)
    }

    /// [`with_policy`](Self::with_policy) with the tier byte budgets and importance weight
    /// the server loaded from its knobs.
    pub fn with_tiers(
        store: Arc<KvStore>,
        instance_id: impl Into<String>,
        policy: TierPolicy,
        budgets: TierBudgets,
    ) -> Self {
        let to_cap = |bytes: u64| usize::try_from(bytes).unwrap_or(usize::MAX);
        let cache = TieredCache::with_cold_store(
            to_cap(budgets.hot_bytes),
            to_cap(budgets.warm_bytes),
            to_cap(budgets.cold_bytes),
            Box::new(KvNamespaceColdStore::new(Arc::clone(&store), BLOCK_NS)),
        )
        .with_retained_cold_copy()
        .with_importance_weight(budgets.importance_weight);
        Self {
            cache: Mutex::new(cache),
            store,
            instance_id: instance_id.into(),
            min_worth: policy.min_worth,
            policy: Mutex::new(PolicyTier::new(policy).with_label("block")),
            get_hits: AtomicU64::new(0),
            get_misses: AtomicU64::new(0),
            dedup_hits: AtomicU64::new(0),
            put_new: AtomicU64::new(0),
            stale_missed: AtomicU64::new(0),
            accesses: AtomicU64::new(0),
        }
    }

    /// `true` when the backing store lands writes durably on disk (a persist dir is
    /// configured) — a fleet whose shared cache survives an engine restart.
    pub fn is_durable(&self) -> bool {
        self.store.is_durable()
    }

    // ── block ops (the SharedKvBackend seam, `&self` internals) ─────────────────────

    /// The tier ladder, recovered if a panic elsewhere left the lock poisoned (the ladder
    /// holds maps and counters only).
    fn cache(&self) -> std::sync::MutexGuard<'_, TieredCache<String, Block>> {
        self.cache
            .lock_recovering("kvcache shared-store tier ladder")
    }

    /// Bring the ladder's data version level with the store's persisted one, so a bump any
    /// instance made retires this instance's stale resident blocks too.
    fn sync_version(&self, cache: &mut TieredCache<String, Block>) {
        let current = self.current_version();
        if cache.current_version() != current {
            cache.set_data_version(current);
        }
    }

    /// Fetch the fresh block stored under `hash`, else `Ok(None)` (absent OR stale); a
    /// page-in whose digest differs from the digest recorded at first write refuses with
    /// [`KvTierError::BlockDigestMismatch`]. A block the ladder does not hold is adopted
    /// from its durable row first. Counts the hit/miss and emits the per-op + periodic
    /// hit-rate `tracing`.
    pub fn fetch_checked(&self, hash: &str) -> Result<Option<Block>, KvTierError> {
        self.fetch_with_neighbors(hash, &[])
    }

    /// [`fetch_checked`](Self::fetch_checked) that, on a hit, pages the COLD blocks named
    /// in `neighbors` (the graph neighbours the request read) up to HOT too
    /// (MEDULLA-DURABLE-KERNEL-R083.6). A miss prefetches nothing.
    pub fn fetch_with_neighbors(
        &self,
        hash: &str,
        neighbors: &[String],
    ) -> Result<Option<Block>, KvTierError> {
        let key = hash.to_string();
        let mut cache = self.cache();
        let resident_before = cache.tier_of(&key).is_some();
        self.sync_version(&mut cache);
        let mut stale = false;
        let found = match self.page_in(&mut cache, &key, &mut stale, resident_before) {
            Ok(found) => found,
            Err(refusal) => {
                drop(cache);
                self.miss(hash, false);
                return Err(refusal);
            }
        };
        if let Some(block) = &found {
            for neighbor in neighbors.iter().filter(|neighbor| **neighbor != key) {
                cache.adopt_cold(neighbor);
                if cache.tier_of(neighbor) == Some(Tier::Cold) {
                    // A cold neighbour pages up to HOT; a digest refusal drops it and
                    // the neighbour's own read refuses later, so it is not this read's.
                    let _ = cache.try_get(neighbor);
                }
            }
            drop(cache);
            self.note_served(hash, block.len() as u64);
            self.get_hits.fetch_add(1, Ordering::Relaxed);
            self.trace_access(hash, true);
            self.maybe_log_rate();
            return Ok(found);
        }
        drop(cache);
        if stale {
            self.stale_missed.fetch_add(1, Ordering::Relaxed);
        }
        Ok(self.miss(hash, stale))
    }

    /// Serve `key` from the ladder, adopting its durable row when no tier holds it.
    /// `stale` is set when the miss is a version-retired block, not an absent one.
    fn page_in(
        &self,
        cache: &mut TieredCache<String, Block>,
        key: &String,
        stale: &mut bool,
        resident_before: bool,
    ) -> Result<Option<Block>, KvTierError> {
        if let Some(block) = cache.try_get(key)? {
            return Ok(Some(block));
        }
        if resident_before {
            *stale = true;
        }
        let raw = match self.store.get(BLOCK_NS, key) {
            Ok(Some(raw)) => raw,
            Ok(None) => return Ok(None),
            Err(error) => {
                // A store read error is a cache MISS (never crash token generation), but
                // surface it: it is not a normal absent-key outcome.
                tracing::warn!(
                    target: "medulla::kvcache",
                    instance = %self.instance_id,
                    hash = %key,
                    error = %error,
                    "kvcache durable get failed, treating as miss"
                );
                return Ok(None);
            }
        };
        let Some(frame) = decode_cold_frame(&raw) else {
            tracing::warn!(
                target: "medulla::kvcache",
                instance = %self.instance_id,
                hash = %key,
                "kvcache stored block frame is malformed, treating as miss"
            );
            return Ok(None);
        };
        if !frame.version.is_fresh(cache.current_version()) {
            // A row derived at a version the store has moved past: retire it.
            *stale = true;
            if let Err(error) = self.store.delete(BLOCK_NS, key) {
                tracing::warn!(
                    target: "medulla::kvcache",
                    instance = %self.instance_id,
                    hash = %key,
                    error = %error,
                    "kvcache could not delete a stale block row"
                );
            }
            return Ok(None);
        }
        if cache.adopt_cold(key) {
            return cache.try_get(key);
        }
        Ok(None)
    }

    fn miss(&self, hash: &str, _stale: bool) -> Option<Block> {
        {
            let mut policy = self.ledger();
            if policy.contains(SHARED_TENANT, hash) {
                // The ledger still holds a block the store no longer serves.
                policy.invalidate(SHARED_TENANT, hash);
            } else {
                policy.lookup(SHARED_TENANT, hash, 0);
            }
        }
        self.get_misses.fetch_add(1, Ordering::Relaxed);
        self.trace_access(hash, false);
        self.maybe_log_rate();
        None
    }

    /// Store `block` under `hash`, framed with the [`DataVersion`] it was derived at.
    /// Returns `Ok(true)` iff a NEW entry was created; `Ok(false)` on a dedup hit against
    /// an already-present block (the cross-instance / prefix-cache win) or when the tier
    /// policy refused the block (a refusal is a miss, not an error). A durable write
    /// failure is surfaced as `Err` so the HTTP surface can 500 rather than silently
    /// claim the block is cached. Public so the HTTP `KvCacheStore` (which holds this
    /// backend by shared `&self`) can route a PUT here without the trait's `&mut self`.
    pub fn store_block(
        &self,
        hash: &str,
        block: Block,
        derived_at: DataVersion,
    ) -> Result<bool, BackendError> {
        Ok(matches!(
            self.store_block_priced(hash, block, derived_at, None)?,
            StoreOutcome::Created
        ))
    }

    /// [`store_block`](Self::store_block) with the caller's claim of what rebuilding the
    /// block would cost. A block admitted without a claim is priced at the worth floor.
    pub fn store_block_priced(
        &self,
        hash: &str,
        block: Block,
        derived_at: DataVersion,
        rebuild_cost_ms: Option<u64>,
    ) -> Result<StoreOutcome, BackendError> {
        // Whether a row occupying this key is FRESH right now — not merely whether
        // SOME row physically occupies it. An `Agnostic` page is fresh forever
        // (content-addressed: any present row is byte-identical), so this matches
        // the old raw-presence check for that case unchanged. A versioned `At(v)`
        // entry, though, can legitimately be a DIFFERENT value at a new version —
        // that is the whole point of `DataVersion::At` — so a STALE row sitting
        // under this key is superseded content, not a duplicate of what's being
        // written now: a re-publish over it must count as a fresh creation, not a
        // dedup (`version_bump_invalidates_derived_context_fleet_wide` proves the
        // fleet-wide-miss → re-publish round trip; treating that re-publish as a
        // "dedup" both under-reports `put_new` and — via the `false` return this
        // whole method surfaces to callers as "not created" — makes a real refresh
        // of a genuinely fresh value indistinguishable from restoring a stale one).
        let existing_is_fresh = self
            .store
            .get(BLOCK_NS, hash)
            .ok()
            .flatten()
            .as_deref()
            .and_then(decode_cold_frame)
            .is_some_and(|frame| frame.version.is_fresh(self.current_version()));
        // A content-addressed page already present (and therefore fresh) is
        // identical bytes — skip the redundant durable rewrite and count the
        // cross-instance dedup (the hot path).
        if existing_is_fresh && derived_at.is_agnostic() {
            self.dedup_hits.fetch_add(1, Ordering::Relaxed);
            self.trace_put(hash, false);
            return Ok(StoreOutcome::Deduplicated);
        }
        // The tier policy decides whether the block is kept at all, and which
        // lower-worth blocks leave to make room.
        let evicted = match self.admit(hash, block.len() as u64, rebuild_cost_ms) {
            Ok(evicted) => evicted,
            Err(refusal) => {
                self.trace_put(hash, false);
                return Ok(StoreOutcome::Refused(refusal));
            }
        };
        let mut cache = self.cache();
        self.sync_version(&mut cache);
        // Write through to the durable row before anything is acknowledged: the cold
        // rung's namespace is the one durable copy of the block, so a block that is HOT
        // only in RAM never exists without it.
        let stored = StoredBlock::encode_with(cache.warm_codec(), &block);
        let frame = encode_cold_frame(&ColdFrame {
            codec: stored.codec,
            orig_len: stored.orig_len,
            version: derived_at,
            digest: content_hash(&block),
            data: stored.data,
        });
        self.store
            .put(BLOCK_NS, hash, frame)
            .map_err(BackendError::Store)?;
        for gone in evicted {
            cache.evict(&gone.key_digest);
            if let Err(error) = self.store.delete(BLOCK_NS, &gone.key_digest) {
                tracing::warn!(
                    target: "medulla::kvcache",
                    instance = %self.instance_id,
                    hash = %gone.key_digest,
                    error = %error,
                    "kvcache could not delete a policy-evicted block"
                );
            }
        }
        cache.try_put_versioned(hash.to_string(), block, derived_at)?;
        drop(cache);
        if existing_is_fresh {
            // A versioned re-publish of an ALREADY-fresh value REFRESHES the
            // entry's version stamp (un-stales it) — still a dedup.
            self.dedup_hits.fetch_add(1, Ordering::Relaxed);
            self.trace_put(hash, false);
            Ok(StoreOutcome::Deduplicated)
        } else {
            self.put_new.fetch_add(1, Ordering::Relaxed);
            self.trace_put(hash, true);
            Ok(StoreOutcome::Created)
        }
    }

    fn ledger(&self) -> std::sync::MutexGuard<'_, PolicyTier> {
        // The ledger holds counters and heap entries only; a panic elsewhere leaves it
        // consistent enough to keep serving.
        self.policy
            .lock_recovering("kvcache shared-store policy ledger")
    }

    /// The meta a block of `bytes` is admitted under. With no claim the rebuild cost is
    /// the smallest whole number of milliseconds that reaches the worth floor.
    fn block_meta(&self, hash: &str, bytes: u64, rebuild_cost_ms: Option<u64>) -> CacheEntryMeta {
        let rebuild_cost_ms = rebuild_cost_ms
            .unwrap_or_else(|| (self.min_worth * bytes as f64 * (1.0 + 1e-9)).ceil() as u64);
        CacheEntryMeta {
            key_digest: hash.to_string(),
            tier: Tier::Warm,
            bytes,
            // The publish that carries the block is its first use.
            hits: 1,
            rebuild_cost_ms,
            deps: Vec::new(),
        }
    }

    /// Offer one block to the tier; returns the metadata of the blocks it evicted.
    fn admit(
        &self,
        hash: &str,
        bytes: u64,
        rebuild_cost_ms: Option<u64>,
    ) -> Result<Vec<CacheEntryMeta>, AdmissionRefusal> {
        let meta = self.block_meta(hash, bytes, rebuild_cost_ms);
        self.ledger()
            .admit_reporting(SHARED_TENANT, meta, Vec::new(), u64::MAX)
    }

    /// Record a served block as a hit. A block stored before this process started has
    /// no ledger entry yet; it is adopted at the worth floor first.
    fn note_served(&self, hash: &str, bytes: u64) {
        let mut policy = self.ledger();
        if !policy.contains(SHARED_TENANT, hash) {
            let meta = self.block_meta(hash, bytes, None);
            // An adopted block the tier refuses (it alone exceeds the budget) is still
            // served this once; it simply is not ledgered.
            let _ = policy.admit_reporting(SHARED_TENANT, meta, Vec::new(), u64::MAX);
            return;
        }
        policy.lookup(SHARED_TENANT, hash, 0);
    }

    /// Whether a fresh (non-stale) block exists under `hash`: its durable row is the
    /// record of every block the fleet holds.
    fn has(&self, hash: &str) -> bool {
        match self.store.get(BLOCK_NS, hash) {
            Ok(Some(raw)) => decode_cold_frame(&raw)
                .is_some_and(|frame| frame.version.is_fresh(self.current_version())),
            _ => false,
        }
    }

    /// Advance the store's current data version (CONCEPT:MEDULLA-KG.storage.content-addressed-put),
    /// persisting it so the WHOLE fleet gates freshness against the same epoch. Stale
    /// entries are then read-gated lazily (see [`retire_stale`](Self::retire_stale) for
    /// bounded disk reclamation). Passing `Agnostic` clears tracking. Public so the HTTP
    /// `KvCacheStore` can route a version bump here without the trait's `&mut self`.
    pub fn set_version(&self, version: DataVersion) -> Result<(), String> {
        match version {
            DataVersion::At(v) => self
                .store
                .put(META_NS, VERSION_KEY, v.to_be_bytes().to_vec())?,
            DataVersion::Agnostic => {
                self.store.delete(META_NS, VERSION_KEY)?;
            }
        }
        let mut cache = self.cache();
        self.sync_version(&mut cache);
        Ok(())
    }

    /// The store's current data version (`Agnostic` when tracking is inactive).
    pub fn current_version(&self) -> DataVersion {
        match self.store.get(META_NS, VERSION_KEY) {
            Ok(Some(bytes)) if bytes.len() == 8 => {
                let mut buf = [0u8; 8];
                buf.copy_from_slice(&bytes);
                DataVersion::At(u64::from_be_bytes(buf))
            }
            _ => DataVersion::Agnostic,
        }
    }

    /// Bounded reclamation sweep: scan up to `max` blocks and DELETE the ones now stale
    /// under the current version, returning how many were retired. The opt-in,
    /// out-of-band disk counterpart to `SharedKvIndex`'s eager in-RAM retirement. Pure
    /// content-addressed (`Agnostic`) pages are never retired.
    pub fn retire_stale(&self, max: usize) -> Result<usize, String> {
        let current = self.current_version();
        if current.is_agnostic() {
            return Ok(0); // tracking inactive ⇒ nothing is stale
        }
        let pairs = self.store.scan(BLOCK_NS, "", max)?;
        let mut retired = 0usize;
        for (key, raw) in pairs {
            if let Some(frame) = decode_cold_frame(&raw) {
                if matches!(frame.version, DataVersion::At(_))
                    && !frame.version.is_fresh(current)
                    && self.store.delete(BLOCK_NS, &key)?
                {
                    self.cache().evict(&key);
                    retired += 1;
                }
            }
        }
        if retired > 0 {
            tracing::debug!(
                target: "medulla::kvcache",
                instance = %self.instance_id,
                retired,
                "kvcache retired stale derived-context blocks"
            );
        }
        Ok(retired)
    }

    /// Graph-topology-aware paging (CONCEPT:MEDULLA-KG.memory.graph-guided-paging): page the
    /// top-`budget` PRESENT blocks from the shared durable store into RAM, in the caller's
    /// graph-locality order. `ranked_keys` is a block-key ranking a caller derives from the
    /// resident graph's own topology (e.g. `medulla_graph_algos::graph_importance::pagerank_importance`
    /// over a `GraphView`, mapped node-id → block-key) — so the KV blocks of the most
    /// graph-central nodes are the ones paged into the fast tier first, exactly like
    /// `TieredCache::apply_importance_scores` shapes eviction by centrality. Returns the
    /// `(key, block)` pairs actually resident (misses are skipped), capped at `budget`.
    pub fn page_in_ranked(&self, ranked_keys: &[String], budget: usize) -> Vec<(String, Block)> {
        let mut paged = Vec::new();
        for key in ranked_keys {
            if paged.len() >= budget {
                break;
            }
            if let Ok(Some(block)) = self.fetch_checked(key) {
                paged.push((key.clone(), block));
            }
        }
        paged
    }

    /// Re-score every block by its place in the graph (MEDULLA-DURABLE-KERNEL-R083.6): the
    /// PageRank of the topology `edges` (block hash to block hash) replaces the previous
    /// importance, so the demotion score `recency + importance_weight * importance` keeps
    /// the graph-central blocks resident longer. Returns the number of blocks scored. The
    /// scores are derived and rebuildable, so they live only in the ladder.
    pub fn refresh_importance(&self, edges: &[(String, String)]) -> usize {
        use medulla_graph_core::graph::GraphCore;
        let graph = GraphCore::new();
        let mut seen = HashSet::new();
        for (from, to) in edges {
            for node in [from, to] {
                if seen.insert(node.clone()) {
                    graph.add_node(node.clone(), b"{}".to_vec());
                }
            }
        }
        for (from, to) in edges {
            if let Err(error) = graph.add_edge(from.clone(), to.clone(), b"{}".to_vec()) {
                tracing::warn!(
                    target: "medulla::kvcache",
                    instance = %self.instance_id,
                    error = %error,
                    "kvcache skipped a topology edge"
                );
            }
        }
        let scores = medulla_graph_algos::graph_importance::pagerank_importance(
            &graph.topology_snapshot(),
            0.85,
            20,
        );
        let scored = scores.len();
        let mut cache = self.cache();
        cache.clear_importance();
        cache.apply_importance_scores(scores);
        scored
    }

    /// The tier `hash` currently occupies in this instance's ladder, if resident and fresh.
    pub fn tier_of(&self, hash: &str) -> Option<Tier> {
        self.cache().tier_of(&hash.to_string())
    }

    /// Occupancy and lifetime counters of this instance's tier ladder.
    pub fn tier_stats(&self) -> medulla_kvcache::CacheStats {
        self.cache().stats()
    }

    /// Current occupancy + hit-rate counters (the Seam-6 export shape).
    pub fn stats(&self) -> SharedKvStoreStats {
        SharedKvStoreStats {
            get_hits: self.get_hits.load(Ordering::Relaxed),
            get_misses: self.get_misses.load(Ordering::Relaxed),
            dedup_hits: self.dedup_hits.load(Ordering::Relaxed),
            put_new: self.put_new.load(Ordering::Relaxed),
            stale_missed: self.stale_missed.load(Ordering::Relaxed),
        }
    }

    /// The instance id stamped on this backend's metrics/logs.
    pub fn instance_id(&self) -> &str {
        &self.instance_id
    }

    // ── tracing (GOOD LOGS: per-op + periodic hit-rate, never println!) ─────────────

    fn trace_access(&self, hash: &str, hit: bool) {
        tracing::trace!(
            target: "medulla::kvcache",
            instance = %self.instance_id,
            hash = %hash,
            hit,
            "kvcache get"
        );
    }

    fn trace_put(&self, hash: &str, created: bool) {
        tracing::trace!(
            target: "medulla::kvcache",
            instance = %self.instance_id,
            hash = %hash,
            created,
            "kvcache put"
        );
    }

    /// Periodic hit-rate metric, once every [`HIT_RATE_LOG_EVERY`] accesses — the SAME
    /// shape as `result_cache::maybe_log_rate` (W3.7 Seam-6): `hits`/`misses`/`hit_rate`
    /// under `target: "medulla::kvcache"`.
    fn maybe_log_rate(&self) {
        let n = self.accesses.fetch_add(1, Ordering::Relaxed) + 1;
        if !n.is_multiple_of(HIT_RATE_LOG_EVERY) {
            return;
        }
        let stats = self.stats();
        tracing::debug!(
            target: "medulla::kvcache",
            instance = %self.instance_id,
            hits = stats.get_hits,
            misses = stats.get_misses,
            dedup_hits = stats.dedup_hits,
            stale_missed = stats.stale_missed,
            hit_rate = stats.hit_rate(),
            "kvcache hit-rate metric"
        );
    }
}

impl SharedKvBackend for SharedKvStoreBackend {
    fn get_block(&self, hash: &str) -> Option<Block> {
        self.fetch_checked(hash).ok().flatten()
    }

    fn put_block(&mut self, hash: &str, block: Block, derived_at: DataVersion) -> bool {
        // Graceful degradation for the generic seam (LMCache treats a failed offload as a
        // non-fatal miss): a durable write error becomes `false` (not-created) + a warn.
        match self.store_block(hash, block, derived_at) {
            Ok(created) => created,
            Err(error) => {
                tracing::warn!(
                    target: "medulla::kvcache",
                    instance = %self.instance_id,
                    hash = %hash,
                    error = %error,
                    "kvcache durable put failed"
                );
                false
            }
        }
    }

    fn contains(&self, hash: &str) -> bool {
        self.has(hash)
    }

    fn set_data_version(&mut self, version: DataVersion) {
        if let Err(error) = self.set_version(version) {
            tracing::warn!(
                target: "medulla::kvcache",
                instance = %self.instance_id,
                error = %error,
                "kvcache set_data_version failed"
            );
        }
    }
}

/// Order block keys by a graph-locality importance ranking (CONCEPT:MEDULLA-KG.memory.graph-guided-paging),
/// highest-importance first, dropping any key that has no mapping. The convenience the
/// snapshot/page-in path uses to page graph-central blocks in first: hand it a
/// `node-id → score` ranking (`pagerank_importance`/`degree_centrality_importance`) plus a
/// `node-id → block-key` map. Kept here (not in the leaf crate) because the ranking is
/// produced by the facade's already-linked `medulla-compute` graph algorithms.
pub fn rank_keys_by_importance<F>(scores: &[(String, f64)], key_of: F) -> Vec<String>
where
    F: Fn(&str) -> Option<String>,
{
    let mut ranked: Vec<(String, f64)> = scores
        .iter()
        .filter_map(|(node, score)| key_of(node).map(|key| (key, *score)))
        .collect();
    // Descending importance; NaN-safe (treat unordered as equal).
    ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    let mut seen = HashSet::new();
    ranked
        .into_iter()
        .map(|(key, _)| key)
        .filter(|key| seen.insert(key.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store_path(tag: &str) -> std::path::PathBuf {
        let unique = format!(
            "medulla-kvcache-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let dir = std::env::temp_dir().join(unique);
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn store(tag: &str) -> Arc<KvStore> {
        let dir = store_path(&format!("shared-{tag}"));
        Arc::new(KvStore::open(Some(dir.to_str().unwrap())).unwrap())
    }

    /// CONCEPT:MEDULLA-KG.backend.networked-shared-kv — THE acceptance proof: two serving
    /// instances (A and B) over ONE durable engine store. A block A PUTs is a cache HIT
    /// for B — cross-instance prefix reuse through the engine — and B's get is measured.
    #[test]
    fn cross_instance_shared_prefix_hit_through_the_engine() {
        let shared = store("xinst");
        let mut instance_a = SharedKvStoreBackend::new(shared.clone(), "A");
        let instance_b = SharedKvStoreBackend::new(shared.clone(), "B");

        let prefix_hash = "tok:shared-system-prompt-prefix";
        let kv_page: Block = vec![7u8; 4096];

        // B has never seen the prefix ⇒ a MISS (would recompute).
        assert_eq!(instance_b.get_block(prefix_hash), None);
        assert!(!instance_b.contains(prefix_hash));

        // A (the warm worker) computes + PUTs the prefix block.
        assert!(
            instance_a.put_block(prefix_hash, kv_page.clone(), DataVersion::Agnostic),
            "A stores a brand-new block"
        );

        // B (a cold worker) now HITs the SAME prefix through the engine — no recompute.
        assert!(
            instance_b.contains(prefix_hash),
            "B sees A's block via the shared store"
        );
        assert_eq!(
            instance_b.get_block(prefix_hash),
            Some(kv_page),
            "the block A wrote is a cross-instance HIT for B"
        );

        // Measured: B recorded exactly one miss (the cold probe) then one hit.
        let bstats = instance_b.stats();
        assert_eq!(bstats.get_hits, 1, "B's cross-instance hit is counted");
        assert_eq!(bstats.get_misses, 1, "B's initial cold miss is counted");
        assert!((bstats.hit_rate() - 0.5).abs() < 1e-9);
        // A stored one new block, no dedup yet.
        assert_eq!(instance_a.stats().put_new, 1);
    }

    /// CONCEPT:MEDULLA-KG.backend.networked-shared-kv — a second instance PUTting the SAME
    /// content-addressed block dedups (the shared store already has it) rather than
    /// re-storing, and the dedup is counted (the cross-instance store-once win).
    #[test]
    fn second_instance_put_dedups_against_the_shared_store() {
        let shared = store("dedup");
        let mut a = SharedKvStoreBackend::new(shared.clone(), "A");
        let mut b = SharedKvStoreBackend::new(shared.clone(), "B");
        let hash = "tok:hot-prefix";
        let page: Block = vec![3u8; 512];

        assert!(
            a.put_block(hash, page.clone(), DataVersion::Agnostic),
            "A: new"
        );
        assert!(
            !b.put_block(hash, page.clone(), DataVersion::Agnostic),
            "B: same content already in the shared store ⇒ dedup, not new"
        );
        assert_eq!(b.stats().dedup_hits, 1);
        assert_eq!(b.stats().put_new, 0);
    }

    /// CONCEPT:MEDULLA-KG.storage.content-addressed-put — a versioned derived-context entry is
    /// served, then a `set_data_version` bump (a graph write, PERSISTED so all instances
    /// agree) makes it a stale MISS on ANOTHER instance; a re-publish at the new version
    /// serves fresh. Agnostic pages are immune.
    #[test]
    fn version_bump_invalidates_derived_context_fleet_wide() {
        let shared = store("ver");
        let mut writer = SharedKvStoreBackend::new(shared.clone(), "writer");
        let reader = SharedKvStoreBackend::new(shared.clone(), "reader");

        writer.set_data_version(DataVersion::At(0));
        assert!(writer.put_block("agent-ctx", b"ctx-at-v0".to_vec(), DataVersion::At(0)));
        // A pure KV page alongside it (must survive the bump).
        assert!(writer.put_block("pure-page", vec![1u8; 64], DataVersion::Agnostic));

        // The READER instance (separate handle) sees the fresh context via the shared store.
        assert_eq!(
            reader.get_block("agent-ctx").as_deref(),
            Some(&b"ctx-at-v0"[..])
        );

        // A graph write bumps the epoch (persisted) ⇒ the reader now MISSES the stale ctx…
        writer.set_data_version(DataVersion::At(1));
        assert_eq!(
            reader.get_block("agent-ctx"),
            None,
            "stale context is a fleet-wide miss"
        );
        assert!(!reader.contains("agent-ctx"));
        assert_eq!(reader.stats().stale_missed, 1, "the stale miss is counted");
        // …but the pure content-addressed page is untouched.
        assert_eq!(reader.get_block("pure-page"), Some(vec![1u8; 64]));

        // Re-publish freshly-derived context at v1 ⇒ served again.
        assert!(writer.put_block("agent-ctx", b"ctx-at-v1".to_vec(), DataVersion::At(1)));
        assert_eq!(
            reader.get_block("agent-ctx").as_deref(),
            Some(&b"ctx-at-v1"[..])
        );

        // retire_stale is a no-op now (nothing stale at v1), and it never touches pure pages.
        assert_eq!(writer.retire_stale(1000).unwrap(), 0);
        assert!(writer.contains("pure-page"));
    }

    /// CONCEPT:MEDULLA-KG.storage.content-addressed-put — `retire_stale` reclaims stale
    /// derived-context blocks from disk (the bounded, out-of-band sweep) while leaving
    /// fresh + agnostic entries.
    #[test]
    fn retire_stale_reclaims_only_stale_derived_blocks() {
        let shared = store("retire");
        let mut b = SharedKvStoreBackend::new(shared.clone(), "s");
        b.set_data_version(DataVersion::At(5));
        assert!(b.put_block("stale-a", b"x".to_vec(), DataVersion::At(4))); // arrives stale
        assert!(b.put_block("stale-b", b"y".to_vec(), DataVersion::At(3))); // arrives stale
        assert!(b.put_block("fresh", b"z".to_vec(), DataVersion::At(5)));
        assert!(b.put_block("pure", vec![9u8; 8], DataVersion::Agnostic));

        let retired = b.retire_stale(1000).unwrap();
        assert_eq!(retired, 2, "both stale derived blocks retired");
        assert!(b.contains("fresh"));
        assert!(b.contains("pure"));
        assert_eq!(b.get_block("stale-a"), None);
    }

    /// CONCEPT:MEDULLA-KG.memory.graph-guided-paging — graph topology decides paging over the
    /// SHARED backend: PageRank over a star graph ranks the hub above the leaves, so
    /// `page_in_ranked` with a budget of 1 pages in the hub's KV block (the graph-central
    /// one), NOT a peripheral leaf's.
    #[test]
    fn graph_topology_pages_central_blocks_in_first() {
        use medulla_graph_algos::algorithms::pagerank;
        use medulla_graph_core::graph::GraphCore;

        // A star graph: every leaf cites the hub.
        let g = GraphCore::new();
        g.add_node("hub".to_string(), b"{}".to_vec());
        for i in 0..6 {
            let leaf = format!("leaf{i}");
            g.add_node(leaf.clone(), b"{}".to_vec());
            g.add_edge(leaf, "hub".to_string(), b"{}".to_vec()).unwrap();
        }
        let scores = pagerank(&g.topology_snapshot(), 0.85, 20);

        // Every node's KV block lives in the shared store, keyed "kv:<node>".
        let shared = store("locality");
        let mut backend = SharedKvStoreBackend::new(shared.clone(), "s");
        for (node, _) in &scores {
            assert!(backend.put_block(
                &format!("kv:{node}"),
                vec![0u8; 128],
                DataVersion::Agnostic
            ));
        }

        // Rank block-keys by graph importance, then page in only the single most central.
        let ranked = rank_keys_by_importance(&scores, |node| Some(format!("kv:{node}")));
        assert_eq!(
            ranked.first().map(String::as_str),
            Some("kv:hub"),
            "hub ranks first"
        );
        let paged = backend.page_in_ranked(&ranked, 1);
        assert_eq!(paged.len(), 1);
        assert_eq!(
            paged[0].0, "kv:hub",
            "the graph-central block is paged in first"
        );
    }

    /// Durability: a block one instance PUTs survives a store close + reopen, so the fleet
    /// cache survives an engine restart (the L2 durability property).
    #[test]
    fn shared_block_survives_store_reopen() {
        let dir = store_path("reopen");
        let path = dir.to_str().unwrap();
        {
            let s = Arc::new(KvStore::open(Some(path)).unwrap());
            let mut a = SharedKvStoreBackend::new(s, "A");
            assert!(a.put_block("survivor", vec![5u8; 256], DataVersion::Agnostic));
        }
        // A "restarted" engine reopens the SAME kv.redb — the shared block is still there.
        let s = Arc::new(KvStore::open(Some(path)).unwrap());
        let b = SharedKvStoreBackend::new(s, "B");
        assert_eq!(b.get_block("survivor"), Some(vec![5u8; 256]));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A page whose consecutive bytes differ, so the warm codec cannot shrink it.
    fn page(seed: u8, len: usize) -> Block {
        (0..len)
            .map(|i| (usize::from(seed) * 31 + i * 7) as u8)
            .collect()
    }

    fn small_tiers(store: &Arc<KvStore>, name: &str) -> SharedKvStoreBackend {
        tiers_with_hot(store, name, 100)
    }

    fn tiers_with_hot(store: &Arc<KvStore>, name: &str, hot_bytes: u64) -> SharedKvStoreBackend {
        SharedKvStoreBackend::with_tiers(
            Arc::clone(store),
            name,
            DEFAULT_BLOCK_POLICY,
            TierBudgets {
                hot_bytes,
                warm_bytes: 100,
                cold_bytes: 1_000_000,
                importance_weight: 1.0,
            },
        )
    }

    // spec: MEDULLA-DURABLE-KERNEL-R083.5
    /// Byte pressure demotes blocks hot, warm, cold; a hit pages one back to HOT
    /// byte-identical, and every block keeps its durable row.
    #[test]
    fn over_budget_puts_demote_and_a_get_promotes() {
        let shared = store("tiers");
        let backend = small_tiers(&shared, "A");
        for seed in 0..6u8 {
            backend
                .store_block(
                    &format!("blk-{seed}"),
                    page(seed, 80),
                    DataVersion::Agnostic,
                )
                .unwrap();
        }
        let stats = backend.tier_stats();
        assert!(stats.demotions_to_warm > 0, "hot overflowed into warm");
        assert!(stats.demotions_to_cold > 0, "warm overflowed into cold");
        assert!(stats.cold_entries > 0);
        let cold: Vec<String> = (0..6u8)
            .map(|seed| format!("blk-{seed}"))
            .filter(|hash| backend.tier_of(hash) == Some(Tier::Cold))
            .collect();
        assert!(!cold.is_empty());
        for hash in &cold {
            let seed: u8 = hash.trim_start_matches("blk-").parse().unwrap();
            assert_eq!(backend.fetch_checked(hash).unwrap(), Some(page(seed, 80)));
            assert_eq!(backend.tier_of(hash), Some(Tier::Hot), "a hit promotes");
        }
        for seed in 0..6u8 {
            assert!(
                shared
                    .get(BLOCK_NS, &format!("blk-{seed}"))
                    .unwrap()
                    .is_some(),
                "the namespace row is the one durable copy"
            );
        }
    }

    // spec: MEDULLA-DURABLE-KERNEL-R083.5
    /// A second instance, and a restarted one, serve a block they never held from its row.
    #[test]
    fn a_new_instance_adopts_the_block_rows_of_another() {
        let shared = store("adopt");
        let first = small_tiers(&shared, "A");
        for seed in 0..4u8 {
            first
                .store_block(&format!("blk-{seed}"), page(seed, 80), DataVersion::At(0))
                .unwrap();
        }
        let second = small_tiers(&shared, "B");
        for seed in 0..4u8 {
            assert_eq!(
                second.fetch_checked(&format!("blk-{seed}")).unwrap(),
                Some(page(seed, 80))
            );
        }
        // A bump retires them for the second instance's ladder and for the row alike.
        first.set_version(DataVersion::At(1)).unwrap();
        assert_eq!(second.fetch_checked("blk-0").unwrap(), None);
        assert_eq!(second.stats().stale_missed, 1);
    }

    // spec: MEDULLA-DURABLE-KERNEL-R083.6
    /// Re-scoring from the graph keeps the central block resident while peripheral blocks
    /// of the same age go down the ladder, and a cold neighbour named by the request
    /// pages up with the hit.
    #[test]
    fn graph_importance_keeps_the_central_block_hot_and_prefetches_neighbours() {
        let shared = store("importance");
        let backend = tiers_with_hot(&shared, "A", 200);
        let edges: Vec<(String, String)> = (0..4)
            .map(|i| (format!("leaf-{i}"), "hub".to_string()))
            .collect();
        assert!(backend.refresh_importance(&edges) >= 5);
        backend
            .store_block("hub", page(9, 80), DataVersion::Agnostic)
            .unwrap();
        for i in 0..4u8 {
            backend
                .store_block(&format!("leaf-{i}"), page(i, 80), DataVersion::Agnostic)
                .unwrap();
        }
        assert_eq!(
            backend.tier_of("hub"),
            Some(Tier::Hot),
            "central block outlives"
        );
        let cold_leaf = (0..4u8)
            .map(|i| format!("leaf-{i}"))
            .find(|leaf| backend.tier_of(leaf) == Some(Tier::Cold))
            .expect("a peripheral block reached cold");
        let hit = backend
            .fetch_with_neighbors("hub", std::slice::from_ref(&cold_leaf))
            .unwrap();
        assert_eq!(hit, Some(page(9, 80)));
        assert_eq!(
            backend.tier_of(&cold_leaf),
            Some(Tier::Hot),
            "neighbour prefetched"
        );
    }
}
