// spec: MEDULLA-DURABLE-KERNEL-R800.6
//! Nightly receipt for MEDULLA-DURABLE-KERNEL-R800.6: one million admit and lookup
//! operations against a [`PolicyTier`]. Budget: lookup p50 under 5 microseconds,
//! lookup p99 under 50 microseconds, admission overhead under 10 percent of an
//! unpoliced insert. This bench never gates a pull request; it exits non-zero when a
//! budget is missed so the nightly job records the breach.

use std::collections::HashMap;
use std::time::Instant;

use medulla_kvcache::{CacheEntryMeta, PolicyTier, Tier, TierPolicy};

/// The `KnobConfig` defaults of `kvcache.min_worth` and `kvcache.answer_max_bytes`
/// (the cache crate never reads the server config itself).
const MIN_WORTH: f64 = 0.001;
const ANSWER_MAX_BYTES: u64 = 256 * 1024 * 1024;

const OPS: usize = 1_000_000;
const KEYS: usize = 100_000;
const ENTRY_BYTES: u64 = 64;
const FOREVER: u64 = u64::MAX;

const LOOKUP_P50_BUDGET_NS: u64 = 5_000;
const LOOKUP_P99_BUDGET_NS: u64 = 50_000;
const ADMISSION_OVERHEAD_BUDGET: f64 = 0.10;

fn meta(key: &str) -> CacheEntryMeta {
    CacheEntryMeta {
        key_digest: key.to_string(),
        tier: Tier::Hot,
        bytes: ENTRY_BYTES,
        hits: 1,
        rebuild_cost_ms: 10,
        deps: Vec::new(),
    }
}

fn percentile(sorted: &[u64], fraction: f64) -> u64 {
    let index = ((sorted.len() as f64 * fraction) as usize).min(sorted.len() - 1);
    sorted[index]
}

fn main() {
    let keys: Vec<String> = (0..KEYS).map(|index| format!("key-{index:08}")).collect();
    let admits = OPS / 2;
    let lookups = OPS - admits;

    let mut tier = PolicyTier::new(TierPolicy {
        max_bytes: ANSWER_MAX_BYTES,
        min_worth: MIN_WORTH,
    });
    let started = Instant::now();
    for index in 0..admits {
        let key = &keys[index % KEYS];
        tier.admit("tenant", meta(key), Vec::new(), FOREVER)
            .expect("hot entries clear the floor");
    }
    let policed = started.elapsed();

    let mut unpoliced: HashMap<(String, String), (CacheEntryMeta, Vec<u8>)> = HashMap::new();
    let started = Instant::now();
    for index in 0..admits {
        let key = &keys[index % KEYS];
        unpoliced.insert(("tenant".to_string(), key.clone()), (meta(key), Vec::new()));
    }
    let baseline = started.elapsed();

    let mut samples = Vec::with_capacity(lookups);
    for index in 0..lookups {
        let key = &keys[(index * 7) % KEYS];
        let started = Instant::now();
        let served = tier.lookup("tenant", key, 0);
        samples.push(started.elapsed().as_nanos() as u64);
        assert!(served.is_some(), "every key was admitted above");
    }
    samples.sort_unstable();
    let p50 = percentile(&samples, 0.50);
    let p99 = percentile(&samples, 0.99);
    let overhead = policed.as_secs_f64() / baseline.as_secs_f64() - 1.0;

    println!("operations          {OPS} ({admits} admit, {lookups} lookup)");
    println!("lookup p50          {p50} ns (budget {LOOKUP_P50_BUDGET_NS})");
    println!("lookup p99          {p99} ns (budget {LOOKUP_P99_BUDGET_NS})");
    println!(
        "admission overhead  {:.1}% (budget {:.0}%)",
        overhead * 100.0,
        ADMISSION_OVERHEAD_BUDGET * 100.0
    );

    let mut breaches = Vec::new();
    if p50 >= LOOKUP_P50_BUDGET_NS {
        breaches.push("lookup p50");
    }
    if p99 >= LOOKUP_P99_BUDGET_NS {
        breaches.push("lookup p99");
    }
    if overhead >= ADMISSION_OVERHEAD_BUDGET {
        breaches.push("admission overhead");
    }
    if !breaches.is_empty() {
        eprintln!("budget breached: {}", breaches.join(", "));
        std::process::exit(1);
    }
}
