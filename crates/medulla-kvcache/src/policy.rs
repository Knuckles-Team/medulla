//! Worth-based admission and eviction for one cache tier (MEDULLA-DURABLE-KERNEL-R800).
//!
//! `worth = hits * rebuild_cost_ms / bytes`. An entry is admitted only when its
//! worth reaches the tier's `min_worth`; under byte pressure the lowest-worth entry
//! leaves first through a lazily-pruned min-heap, so each admission or hit costs
//! O(log N). An answer entry stops being served once any dependency generation it
//! was built at advances, or once its volatility window passes. Every refusal is a
//! miss with no content: this policy never returns a stale or cross-tenant hit.
//!
//! The wiring into the served answer cache and the shared block store is
//! MEDULLA-DURABLE-KERNEL-R800.3; the decision counters are R800.5.

use std::cmp::{Ordering, Reverse};
use std::collections::{BinaryHeap, HashMap};
use std::sync::OnceLock;

use crate::tiered::Tier;

/// One policy decision, as reported to the installed [`PolicyObserver`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PolicyEvent {
    /// An entry cleared the floor and became resident.
    Admitted,
    /// An admission was refused; nothing was stored.
    Refused,
    /// A resident left to make room.
    Evicted,
    /// A resident was dropped because a dependency advanced or its window passed.
    Invalidated,
    /// A fresh entry was served.
    Hit,
    /// No servable entry existed.
    Miss,
}

impl PolicyEvent {
    /// Every event, in counter order.
    pub const ALL: [PolicyEvent; 6] = [
        Self::Admitted,
        Self::Refused,
        Self::Evicted,
        Self::Invalidated,
        Self::Hit,
        Self::Miss,
    ];

    /// The `event` label of the decision counter.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Admitted => "admitted",
            Self::Refused => "refused",
            Self::Evicted => "evicted",
            Self::Invalidated => "invalidated",
            Self::Hit => "hit",
            Self::Miss => "miss",
        }
    }
}

/// One decision. It names a key digest and a tenant, never a key or a value.
#[derive(Clone, Copy, Debug)]
pub struct PolicyDecision<'a> {
    /// The tier label the deciding [`PolicyTier`] carries.
    pub tier: &'a str,
    /// What was decided.
    pub event: PolicyEvent,
    /// The tenant the entry belongs to (an observer records a digest, not this).
    pub tenant: &'a str,
    /// The entry's key digest.
    pub key_digest: &'a str,
    /// The entry's worth at the decision.
    pub worth: f64,
    /// The closed refusal reason, for a refused admission.
    pub reason: Option<&'static str>,
}

/// Receives every decision of every [`PolicyTier`] in the process.
pub trait PolicyObserver: Send + Sync {
    /// Record one decision. Must not block and must not call back into a tier.
    fn record(&self, decision: &PolicyDecision<'_>);
}

static OBSERVER: OnceLock<Box<dyn PolicyObserver>> = OnceLock::new();

/// Install the process-wide observer; the first install wins and `false` means one
/// was already installed.
pub fn install_observer(observer: Box<dyn PolicyObserver>) -> bool {
    OBSERVER.set(observer).is_ok()
}

fn observe(decision: PolicyDecision<'_>) {
    if let Some(observer) = OBSERVER.get() {
        observer.record(&decision);
    }
}

/// The generation of one dependency an entry was derived from.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct GenerationRef {
    pub source: String,
    pub generation: u64,
}

/// Bookkeeping for one cached entry.
#[derive(Clone, Debug, PartialEq)]
pub struct CacheEntryMeta {
    pub key_digest: String,
    pub tier: Tier,
    pub bytes: u64,
    pub hits: u64,
    pub rebuild_cost_ms: u64,
    pub deps: Vec<GenerationRef>,
}

impl CacheEntryMeta {
    /// `hits * rebuild_cost_ms / bytes`; a zero-byte entry has no worth to weigh.
    pub fn worth(&self) -> f64 {
        if self.bytes == 0 {
            return 0.0;
        }
        self.hits as f64 * self.rebuild_cost_ms as f64 / self.bytes as f64
    }
}

/// The byte budget and admission floor of one tier.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TierPolicy {
    pub max_bytes: u64,
    pub min_worth: f64,
}

/// Why an admission was refused. Each refusal stores nothing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AdmissionRefusal {
    /// The entry alone is larger than the tier.
    Oversize { bytes: u64, max_bytes: u64 },
    /// A dependency names a source the generation clock does not know, or a
    /// generation other than its current one.
    UnknownGeneration(GenerationRef),
    /// The entry's worth is below the tier's `min_worth`.
    BelowMinWorth,
}

impl AdmissionRefusal {
    /// The closed, content-free reason recorded in the decision trace.
    pub fn reason(&self) -> &'static str {
        match self {
            Self::Oversize { .. } => "oversize",
            Self::UnknownGeneration(_) => "unknown_generation",
            Self::BelowMinWorth => "below_min_worth",
        }
    }
}

/// What one lookup found. A stale or expired entry is dropped on the way out and
/// reported as `Invalidated`; both it and `Miss` carry no content.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LookupOutcome {
    /// A fresh entry was served.
    Hit(Vec<u8>),
    /// No entry was resident.
    Miss,
    /// An entry was resident but a dependency generation advanced or its window
    /// passed; it was dropped.
    Invalidated,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Worth(f64);

impl Eq for Worth {}

impl PartialOrd for Worth {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Worth {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0.total_cmp(&other.0)
    }
}

type TenantKey = (String, String);

struct Resident {
    meta: CacheEntryMeta,
    content: Vec<u8>,
    expires_at_ms: u64,
    stamp: u64,
}

/// One tier governed by a [`TierPolicy`].
pub struct PolicyTier {
    label: &'static str,
    policy: TierPolicy,
    generations: HashMap<String, u64>,
    entries: HashMap<TenantKey, Resident>,
    heap: BinaryHeap<Reverse<(Worth, u64, TenantKey)>>,
    used_bytes: u64,
    next_stamp: u64,
}

impl PolicyTier {
    pub fn new(policy: TierPolicy) -> Self {
        Self {
            label: "tier",
            policy,
            generations: HashMap::new(),
            entries: HashMap::new(),
            heap: BinaryHeap::new(),
            used_bytes: 0,
            next_stamp: 0,
        }
    }

    /// Name this tier in the decisions it reports (`answer`, `block`, ...).
    pub fn with_label(mut self, label: &'static str) -> Self {
        self.label = label;
        self
    }

    /// Declare or advance a dependency source's current generation. Entries built
    /// at an older generation stop being served.
    pub fn set_generation(&mut self, source: impl Into<String>, generation: u64) {
        self.generations.insert(source.into(), generation);
    }

    /// [`set_generation`](Self::set_generation) that never moves a source backwards, for
    /// callers that learn generations from concurrent computations.
    pub fn advance_generation(&mut self, source: impl Into<String>, generation: u64) {
        let slot = self.generations.entry(source.into()).or_insert(generation);
        *slot = (*slot).max(generation);
    }

    pub fn used_bytes(&self) -> u64 {
        self.used_bytes
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn contains(&self, tenant: &str, key_digest: &str) -> bool {
        self.entries
            .contains_key(&(tenant.to_string(), key_digest.to_string()))
    }

    /// Admit `content` for `tenant`, valid until `expires_at_ms`. Evicts the
    /// lowest-worth residents until the entry fits.
    pub fn admit(
        &mut self,
        tenant: &str,
        meta: CacheEntryMeta,
        content: Vec<u8>,
        expires_at_ms: u64,
    ) -> Result<(), AdmissionRefusal> {
        self.admit_reporting(tenant, meta, content, expires_at_ms)
            .map(|_| ())
    }

    /// [`admit`](Self::admit) that also returns the metadata of every resident it
    /// evicted to make room, so a caller that keeps the bytes elsewhere can release
    /// them. An entry replaced under its own key is not reported.
    pub fn admit_reporting(
        &mut self,
        tenant: &str,
        meta: CacheEntryMeta,
        content: Vec<u8>,
        expires_at_ms: u64,
    ) -> Result<Vec<CacheEntryMeta>, AdmissionRefusal> {
        if let Some(refusal) = self.refusal_for(&meta) {
            self.report(
                tenant,
                &meta.key_digest,
                PolicyEvent::Refused,
                meta.worth(),
                Some(refusal.reason()),
            );
            return Err(refusal);
        }
        let key = (tenant.to_string(), meta.key_digest.clone());
        self.remove(&key);
        let mut evicted = Vec::new();
        while self.used_bytes + meta.bytes > self.policy.max_bytes {
            match self.evict_lowest() {
                Some(gone) => evicted.push(gone),
                None => break,
            }
        }
        self.used_bytes += meta.bytes;
        let worth = meta.worth();
        let stamp = self.push_heap(&key, worth);
        self.report(tenant, &meta.key_digest, PolicyEvent::Admitted, worth, None);
        self.entries.insert(
            key,
            Resident {
                meta,
                content,
                expires_at_ms,
                stamp,
            },
        );
        Ok(evicted)
    }

    fn refusal_for(&self, meta: &CacheEntryMeta) -> Option<AdmissionRefusal> {
        if meta.bytes > self.policy.max_bytes {
            return Some(AdmissionRefusal::Oversize {
                bytes: meta.bytes,
                max_bytes: self.policy.max_bytes,
            });
        }
        if let Some(dep) = meta.deps.iter().find(|dep| !self.dep_is_current(dep)) {
            return Some(AdmissionRefusal::UnknownGeneration(dep.clone()));
        }
        if meta.worth() < self.policy.min_worth {
            return Some(AdmissionRefusal::BelowMinWorth);
        }
        None
    }

    fn report(
        &self,
        tenant: &str,
        key_digest: &str,
        event: PolicyEvent,
        worth: f64,
        reason: Option<&'static str>,
    ) {
        observe(PolicyDecision {
            tier: self.label,
            event,
            tenant,
            key_digest,
            worth,
            reason,
        });
    }

    /// Serve `tenant`'s entry, counting a hit. A stale, expired or absent entry is
    /// a miss; a stale or expired entry is dropped on the way out.
    pub fn lookup(&mut self, tenant: &str, key_digest: &str, now_ms: u64) -> Option<Vec<u8>> {
        match self.lookup_outcome(tenant, key_digest, now_ms) {
            LookupOutcome::Hit(content) => Some(content),
            LookupOutcome::Miss | LookupOutcome::Invalidated => None,
        }
    }

    /// [`lookup`](Self::lookup) that says why it missed: absent, or dropped because a
    /// dependency generation advanced or the volatility window passed.
    pub fn lookup_outcome(&mut self, tenant: &str, key_digest: &str, now_ms: u64) -> LookupOutcome {
        let key = (tenant.to_string(), key_digest.to_string());
        let fresh = match self.entries.get(&key) {
            None => {
                self.report(tenant, key_digest, PolicyEvent::Miss, 0.0, None);
                return LookupOutcome::Miss;
            }
            Some(resident) => {
                now_ms < resident.expires_at_ms
                    && resident
                        .meta
                        .deps
                        .iter()
                        .all(|dep| self.dep_is_current(dep))
            }
        };
        if !fresh {
            let worth = self.remove(&key).map_or(0.0, |gone| gone.worth());
            self.report(tenant, key_digest, PolicyEvent::Invalidated, worth, None);
            return LookupOutcome::Invalidated;
        }
        let worth = match self.entries.get_mut(&key) {
            Some(resident) => {
                resident.meta.hits += 1;
                resident.meta.worth()
            }
            None => return LookupOutcome::Miss,
        };
        let stamp = self.push_heap(&key, worth);
        self.report(tenant, key_digest, PolicyEvent::Hit, worth, None);
        match self.entries.get_mut(&key) {
            Some(resident) => {
                resident.stamp = stamp;
                LookupOutcome::Hit(resident.content.clone())
            }
            None => LookupOutcome::Miss,
        }
    }

    /// Drop `tenant`'s entry without counting a hit; `true` when one was resident.
    pub fn invalidate(&mut self, tenant: &str, key_digest: &str) -> bool {
        match self.remove(&(tenant.to_string(), key_digest.to_string())) {
            Some(gone) => {
                self.report(
                    tenant,
                    key_digest,
                    PolicyEvent::Invalidated,
                    gone.worth(),
                    None,
                );
                true
            }
            None => false,
        }
    }

    /// Remove and return the lowest-worth resident's metadata.
    pub fn evict_lowest(&mut self) -> Option<CacheEntryMeta> {
        while let Some(Reverse((_, stamp, key))) = self.heap.pop() {
            if self.entries.get(&key).is_some_and(|r| r.stamp == stamp) {
                let gone = self.remove(&key)?;
                self.report(
                    &key.0,
                    &gone.key_digest,
                    PolicyEvent::Evicted,
                    gone.worth(),
                    None,
                );
                return Some(gone);
            }
        }
        None
    }

    fn dep_is_current(&self, dep: &GenerationRef) -> bool {
        self.generations.get(&dep.source) == Some(&dep.generation)
    }

    fn push_heap(&mut self, key: &TenantKey, worth: f64) -> u64 {
        let stamp = self.next_stamp;
        self.next_stamp += 1;
        self.heap.push(Reverse((Worth(worth), stamp, key.clone())));
        stamp
    }

    fn remove(&mut self, key: &TenantKey) -> Option<CacheEntryMeta> {
        let resident = self.entries.remove(key)?;
        self.used_bytes -= resident.meta.bytes;
        Some(resident.meta)
    }
}
