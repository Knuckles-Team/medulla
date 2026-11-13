//! Read-time access check bench (MEDULLA-FEDERATED-QUERY-R703.6).
//!
//! One `filter_hits` call filters 256 hits over 4 `checked_at_read` sources with warm
//! caches, the call `handle_context_compile` makes before it compiles a bundle. The gate
//! key `access_check_256` in `thresholds.json` caps p99 at 1 ms; the p99 gate of
//! `scripts/bench_gate.py` enforces it.
//!
//! Run: `cargo bench -p medulla-plan --bench access_check`.
// spec: MEDULLA-FEDERATED-QUERY-R703.6

use criterion::{criterion_group, criterion_main, Criterion};
use medulla_exec::access_check::{
    filter_hits, AccessCache, AccessCheckAnswer, AccessCheckPort, AccessCheckRequest, AccessHit,
    AccessMode, MAX_CACHE_TTL_MS,
};
use std::hint::black_box;

const HITS: usize = 256;
const SOURCES: usize = 4;
const NOW_MS: u64 = 1_000;

/// A port that allows every record it is asked about.
struct AllowAll;

impl AccessCheckPort for AllowAll {
    fn check(&mut self, request: &AccessCheckRequest) -> Option<(AccessCheckAnswer, u64)> {
        let answer = AccessCheckAnswer {
            allowed: request.record_refs.clone(),
            denied: Vec::new(),
            checked_at: NOW_MS,
        };
        Some((answer, 1))
    }
}

fn hits() -> Vec<AccessHit> {
    (0..HITS)
        .map(|n| AccessHit {
            source: format!("source-{}", n % SOURCES),
            record_ref: format!("record-{n}"),
            mode: AccessMode::CheckedAtRead,
        })
        .collect()
}

fn bench_filter(c: &mut Criterion) {
    let mut cache = AccessCache::new(MAX_CACHE_TTL_MS);
    let mut port = AllowAll;
    // Warm the caches: the first pass sends the checks, every later one reads them.
    let warm = filter_hits(hits(), "principal", &mut port, &mut cache, NOW_MS);
    assert_eq!(warm.kept.len(), HITS);
    c.bench_function("access_check_256", |b| {
        b.iter(|| {
            let out = filter_hits(hits(), "principal", &mut port, &mut cache, NOW_MS + 1);
            assert_eq!(out.checks_sent, 0, "a warm cache sends no check");
            black_box(out.kept.len());
        });
    });
}

criterion_group! {
    name = benches;
    config = Criterion::default().sample_size(100);
    targets = bench_filter
}
criterion_main!(benches);
