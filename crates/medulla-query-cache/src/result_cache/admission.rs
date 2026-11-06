//! Admission of dependency-scoped answers through the KV-cache tier policy
//! (MEDULLA-DURABLE-KERNEL-R800.3).
//!
//! The dependency check (`DepSet` against `DepClock`) decides whether a kept answer is
//! still valid; the tier policy decides whether an answer is kept at all. A hit needs
//! both. This crate reads no environment variable: the server hands the policy over at
//! startup through [`install_tier_policy`], built from `kvcache.min_worth` and
//! `kvcache.answer_max_bytes`.

use std::sync::OnceLock;

use medulla_kvcache::{AdmissionRefusal, CacheEntryMeta, GenerationRef, Tier, TierPolicy};

use crate::dep_scope::{DepProbe, DepSet, Dim};

use super::dependency::DepKey;
use super::ResultCache;

/// The `KnobConfig` defaults of `kvcache.answer_max_bytes` (256 MiB) and
/// `kvcache.min_worth` (0.001): what a cache built before startup installs a policy uses.
pub const DEFAULT_TIER_POLICY: TierPolicy = TierPolicy {
    max_bytes: 268_435_456,
    min_worth: 0.001,
};

static INSTALLED: OnceLock<TierPolicy> = OnceLock::new();

/// Hand the startup-loaded answer-tier policy to every [`ResultCache`] built afterwards;
/// the first install wins.
pub fn install_tier_policy(policy: TierPolicy) {
    let _ = INSTALLED.set(policy);
}

pub(super) fn installed_tier_policy() -> TierPolicy {
    INSTALLED.get().copied().unwrap_or(DEFAULT_TIER_POLICY)
}

/// What the served path knows about one answer when it offers it to the tier.
pub struct DepAdmission<'a> {
    /// The tenant the answer belongs to.
    pub tenant: &'a str,
    /// The measured execution time that recomputing the answer would cost again.
    pub rebuild_cost_ms: u64,
    /// The generation of each dependency the answer was built at.
    pub generations: Vec<GenerationRef>,
}

/// The generation of every graph dimension of `deps` as `probe` sees it now. An
/// embedding generation is validated by the dependency check and has none here.
pub fn dep_generations(deps: &DepSet, probe: &DepProbe<'_>) -> Vec<GenerationRef> {
    deps.dims()
        .iter()
        .filter_map(|dim| {
            probe.generation(dim).map(|generation| GenerationRef {
                source: generation_source(dim),
                generation,
            })
        })
        .collect()
}

pub(super) fn generation_source(dim: &Dim) -> String {
    format!("{dim:?}")
}

pub(super) fn policy_digest(query_hash: u128, actor_scope_hash: u64) -> String {
    format!("{query_hash:032x}-{actor_scope_hash:016x}")
}

fn parse_policy_digest(digest: &str) -> Option<DepKey> {
    let (query, actor) = digest.split_once('-')?;
    Some(DepKey {
        query_hash: u128::from_str_radix(query, 16).ok()?,
        actor_scope_hash: u64::from_str_radix(actor, 16).ok()?,
    })
}

impl ResultCache {
    /// Offer one answer of `bytes` length to the answer tier. `Ok` means keep it; a
    /// refusal means store nothing. Answers the tier evicts for room leave the
    /// dependency namespace with them.
    pub fn admit_dep(
        &self,
        query_hash: u128,
        actor_scope_hash: u64,
        admission: &DepAdmission<'_>,
        bytes: u64,
    ) -> Result<(), AdmissionRefusal> {
        let meta = CacheEntryMeta {
            key_digest: policy_digest(query_hash, actor_scope_hash),
            tier: Tier::Hot,
            bytes,
            // The execution that produced the answer is its first use.
            hits: 1,
            rebuild_cost_ms: admission.rebuild_cost_ms,
            deps: admission.generations.clone(),
        };
        let evicted = {
            let mut policy = self.policy.lock();
            for generation in &admission.generations {
                policy.advance_generation(generation.source.clone(), generation.generation);
            }
            policy.admit_reporting(admission.tenant, meta, Vec::new(), u64::MAX)?
        };
        if !evicted.is_empty() {
            let mut inner = self.inner.lock();
            for gone in evicted {
                let Some(key) = parse_policy_digest(&gone.key_digest) else {
                    continue;
                };
                if let Some(entry) = inner.dependency.map.remove(&key) {
                    inner.dependency.recency.remove(&entry.tick);
                }
            }
        }
        Ok(())
    }
}
