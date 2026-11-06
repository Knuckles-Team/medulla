use medulla_kvcache::{PolicyTier, TierPolicy};

use super::{
    admission::installed_tier_policy, cap_from_env, dependency::DependencyEntries,
    versioned::VersionedEntries, Inner, ResultCache,
};

impl ResultCache {
    pub fn new() -> Self {
        Self::with_policy(installed_tier_policy())
    }

    /// Construct a cache whose answer admission follows `policy`: the byte budget of
    /// the answer tier and the worth floor an answer must clear to be kept.
    pub fn with_policy(policy: TierPolicy) -> Self {
        Self::with_cap_and_policy(cap_from_env(), policy)
    }

    /// Construct a cache with an EXPLICIT capacity, bypassing the env override.
    /// `new()` resolves the env cap once and delegates here; tests use it directly
    /// so cache construction never depends on the process-global
    /// `MEDULLA_RESULT_CACHE_CAP` (which would otherwise let one test's
    /// `set_var`/`remove_var` leak into a concurrently-constructed cache).
    pub fn with_cap(cap: usize) -> Self {
        Self::with_cap_and_policy(cap, installed_tier_policy())
    }

    /// [`with_cap`](Self::with_cap) with an explicit admission policy.
    pub fn with_cap_and_policy(cap: usize, policy: TierPolicy) -> Self {
        ResultCache {
            policy: parking_lot::Mutex::new(PolicyTier::new(policy).with_label("answer")),
            inner: parking_lot::Mutex::new(Inner {
                versioned: VersionedEntries::new(),
                dependency: DependencyEntries::new(),
                clock: 0,
            }),
            cap,
            hits: std::sync::atomic::AtomicU64::new(0),
            misses: std::sync::atomic::AtomicU64::new(0),
        }
    }

    /// Drop every cached result. Called when the graph's whole in-RAM state is
    /// wiped (`clear`/`hibernate`) — those paths don't necessarily bump the version,
    /// so the cache is cleared directly to guarantee no post-wipe stale hit.
    pub fn invalidate_all(&self) {
        let mut inner = self.inner.lock();
        inner.versioned.clear();
        inner.dependency.clear();
    }
}

impl std::fmt::Debug for ResultCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (hits, misses) = self.stats();
        f.debug_struct("ResultCache")
            .field("cap", &self.cap)
            .field("len", &self.len())
            .field("hits", &hits)
            .field("misses", &misses)
            .finish()
    }
}

impl Default for ResultCache {
    fn default() -> Self {
        Self::new()
    }
}
