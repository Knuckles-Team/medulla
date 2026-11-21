//! The pgwire extended query's one prepared read classification
//! (MEDULLA-ENGINE-PARITY-R006.4).
//!
//! The extended protocol executes one prepared template many times with different bound
//! values. The template's statement kind does not depend on the values, so it is classified
//! once through the engine's one [`PlanCache`] (`MEDULLA-DURABLE-KERNEL-R041.2`) and reused
//! until the schema version it was classified under changes. No second cache type exists.

use std::sync::atomic::{AtomicU64, Ordering};

use medulla_query_types::plan_cache_key::{PlanCache, PlanCacheLookup};
use medulla_sql_lang::sql::classify::{classify, StatementKind};
use medulla_types::contract::Digest256;

/// How many template classifications were served from the cache, process wide.
static PLAN_CACHE_HITS: AtomicU64 = AtomicU64::new(0);

/// The number of prepared-template classifications served from a connection's plan cache.
pub fn plan_cache_hits() -> u64 {
    PLAN_CACHE_HITS.load(Ordering::Relaxed)
}

/// One connection's cache of template classifications; a cached refusal is kept as the
/// `Err` text `classify` returned.
pub(super) type KindCache = PlanCache<Result<StatementKind, String>>;

/// The statement kind of `template` under `schema_version`: a cached classification on a
/// `Hit` (counted), otherwise the result of [`classify`], cached for the next execution.
pub(super) fn prepare_kind(
    cache: &mut KindCache,
    template: &str,
    schema_version: u64,
) -> Result<StatementKind, String> {
    let (lookup, kind) = cache.prepare(
        Digest256::sha256(template.as_bytes()),
        schema_version,
        || classify(template),
    );
    if lookup == PlanCacheLookup::Hit {
        PLAN_CACHE_HITS.fetch_add(1, Ordering::Relaxed);
    }
    kind
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-ENGINE-PARITY-R006.4
    #[test]
    fn second_execute_of_one_template_is_a_hit() {
        let mut cache = KindCache::new();
        let template = "SELECT id FROM nodes WHERE id = $1";
        let before = plan_cache_hits();

        let first = prepare_kind(&mut cache, template, 7);
        assert_eq!(plan_cache_hits(), before, "the first execution is a miss");
        let second = prepare_kind(&mut cache, template, 7);
        assert_eq!(plan_cache_hits(), before + 1, "the second execution is a hit");
        assert_eq!(first, second);
        assert_eq!(cache.len(), 1);

        // A schema change invalidates the cached classification: recompiled, not a hit.
        let _ = prepare_kind(&mut cache, template, 8);
        assert_eq!(plan_cache_hits(), before + 1);

        // A template that is not a read is cached as its own kind, never as a `Read`, so the
        // caller executes it through `WireSession::execute`.
        let insert = prepare_kind(&mut cache, "INSERT INTO t (a) VALUES ($1)", 8);
        assert_ne!(insert, Ok(StatementKind::Read));
    }
}
