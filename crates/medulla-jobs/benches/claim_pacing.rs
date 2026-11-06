//! Nightly hot-path proof for claim pacing (MEDULLA-PLACEMENT-R750.753).
//!
//! Drives `ClaimPacer::try_claim` across 64 concurrent classes (one pacer per
//! tenant and class slot). Before timing with criterion it asserts the budgets:
//! p50 below 2 microseconds, p99 below 10 microseconds over 20,000 samples, and
//! pacer memory below 1 MiB.
//!
//! Run: cargo bench -p medulla-jobs claim_pacing

// spec: MEDULLA-PLACEMENT-R750.753

use std::hint::black_box;
use std::time::{Duration, Instant};

use criterion::{criterion_group, criterion_main, Criterion};
use medulla_resource::admission::{ClaimPacer, PriorityClass};

const CLASSES: usize = 64;
const SAMPLES: usize = 20_000;
const P50_BUDGET: Duration = Duration::from_micros(2);
const P99_BUDGET: Duration = Duration::from_micros(10);
const MEMORY_BUDGET_BYTES: usize = 1024 * 1024;

fn pacers() -> Vec<ClaimPacer> {
    (0..CLASSES).map(|_| ClaimPacer::new(64, 32, 0)).collect()
}

fn class_of(slot: usize) -> PriorityClass {
    PriorityClass::ALL[slot % PriorityClass::ALL.len()]
}

fn assert_budgets() {
    let memory = CLASSES * std::mem::size_of::<ClaimPacer>();
    assert!(
        memory < MEMORY_BUDGET_BYTES,
        "pacer memory {memory} B over budget"
    );
    let mut pacers = pacers();
    let mut samples = Vec::with_capacity(SAMPLES);
    for i in 0..SAMPLES {
        let slot = i % CLASSES;
        let started = Instant::now();
        black_box(pacers[slot].try_claim(class_of(slot), (i as u64) * 10));
        samples.push(started.elapsed());
    }
    samples.sort_unstable();
    let p50 = samples[SAMPLES / 2];
    let p99 = samples[SAMPLES * 99 / 100];
    assert!(p50 < P50_BUDGET, "p50 {p50:?} over {P50_BUDGET:?}");
    assert!(p99 < P99_BUDGET, "p99 {p99:?} over {P99_BUDGET:?}");
}

fn bench(c: &mut Criterion) {
    assert_budgets();
    let mut pacers = pacers();
    let mut tick = 0u64;
    c.bench_function("jobs/claim_pacing", |b| {
        b.iter(|| {
            tick += 10;
            let slot = (tick as usize / 10) % CLASSES;
            black_box(pacers[slot].try_claim(class_of(slot), tick))
        })
    });
}

criterion_group!(benches, bench);
criterion_main!(benches);
