//! Nightly hot-path proof for baseline planning (MEDULLA-REPO-INGEST-R650.6).
//!
//! Plans 50,000 files against a previous manifest with 5 percent changed.
//! Before timing with criterion it asserts the budgets: p50 <= 10 ms,
//! p99 <= 60 ms over 200 samples, and peak heap <= 48 MiB per plan (counted
//! by a tracking global allocator).
//!
//! Run: cargo bench -p medulla-durable baseline_plan_and_delta

// spec: MEDULLA-REPO-INGEST-R650.6

use std::alloc::{GlobalAlloc, Layout, System};
use std::hint::black_box;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use criterion::{criterion_group, criterion_main, Criterion};
use medulla_durable::{
    plan_baseline, BaselineSources, DeltaManifest, DeltaManifestEntry, DeltaManifestKey,
};
use medulla_types::scheduler_config::SchedulerConfig;

const FILES: usize = 50_000;
const P50_BUDGET: Duration = Duration::from_millis(10);
const P99_BUDGET: Duration = Duration::from_millis(60);
const PEAK_BUDGET_BYTES: usize = 48 * 1024 * 1024;
const SAMPLES: usize = 200;

struct Tracking;

static CURRENT: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for Tracking {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            let now = CURRENT.fetch_add(layout.size(), Ordering::Relaxed) + layout.size();
            PEAK.fetch_max(now, Ordering::Relaxed);
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) };
        CURRENT.fetch_sub(layout.size(), Ordering::Relaxed);
    }
}

#[global_allocator]
static GLOBAL: Tracking = Tracking;

fn entry(n: usize, hash: &str) -> DeltaManifestEntry {
    DeltaManifestEntry {
        key: DeltaManifestKey {
            graph: "workspace".into(),
            category: "source".into(),
            uri: format!("repo://bench/dir{:03}/file{n:06}.rs", n % 500),
        },
        content_hash: format!("sha256:{hash}{n:058}"),
    }
}

fn fixture() -> (BaselineSources, SchedulerConfig) {
    let entries: Vec<DeltaManifestEntry> = (0..FILES).map(|n| entry(n, "c")).collect();
    let previous = DeltaManifest::new(
        "tenant-bench",
        (0..FILES)
            .map(|n| entry(n, if n % 20 == 0 { "p" } else { "c" }))
            .collect(),
    )
    .expect("fixture manifest is valid");
    let mut config: SchedulerConfig = serde_json::from_value(serde_json::json!({
        "total_workers": 4,
        "max_queue_depth": 100
    }))
    .expect("fixture scheduler config is valid");
    config.hydration_scope_cap = 1_000_000;
    let sources = BaselineSources {
        tenant_id: "tenant-bench".into(),
        scope: "workspace".into(),
        entries,
        previous: Some(previous),
    };
    (sources, config)
}

fn assert_budgets(sources: &BaselineSources, config: &SchedulerConfig) {
    let mut samples = Vec::with_capacity(SAMPLES);
    let mut peak = 0usize;
    for _ in 0..SAMPLES {
        let base = CURRENT.load(Ordering::Relaxed);
        PEAK.store(base, Ordering::Relaxed);
        let start = Instant::now();
        let plan = plan_baseline(black_box(sources), config).expect("plan succeeds");
        samples.push(start.elapsed());
        assert_eq!(plan.changed.len(), FILES / 20);
        drop(plan);
        peak = peak.max(PEAK.load(Ordering::Relaxed).saturating_sub(base));
    }
    samples.sort();
    let p50 = samples[SAMPLES / 2];
    let p99 = samples[SAMPLES * 99 / 100];
    assert!(p50 <= P50_BUDGET, "p50 {p50:?} exceeds {P50_BUDGET:?}");
    assert!(p99 <= P99_BUDGET, "p99 {p99:?} exceeds {P99_BUDGET:?}");
    assert!(
        peak <= PEAK_BUDGET_BYTES,
        "peak heap {peak} bytes exceeds {PEAK_BUDGET_BYTES}"
    );
}

fn baseline_plan_and_delta(c: &mut Criterion) {
    let (sources, config) = fixture();
    assert_budgets(&sources, &config);
    c.bench_function("baseline_plan_and_delta", |b| {
        b.iter(|| plan_baseline(black_box(&sources), &config).expect("plan succeeds"))
    });
}

criterion_group!(benches, baseline_plan_and_delta);
criterion_main!(benches);
