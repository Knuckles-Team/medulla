//! Nightly media-facts qualification (MEDULLA-DECISION-ENGINE-R050.3).
//!
//! Four media types import with 128 parts each through
//! `ModelInputCapabilities::validate_parts`. One profile per media type declares the
//! 128-part, 64 MiB ceiling; the run checks the metadata stays within 64 MiB of
//! resident growth and records host, latency (p50, p99) and allocation counts as the
//! group `media_facts`.
//!
//! When `DECIDE_BUDGETS_REPORT` names a report written by the `decide_budgets` bench,
//! the group is merged into it, so `scripts/check_decide_budgets.py` reports it
//! against `benches/baselines/decide.json` in the same nightly run (R178.3).
//! Benchmarks never gate a PR.
//!
//! Run: `cargo bench --bench media_facts`

use std::alloc::{GlobalAlloc, Layout, System};
use std::hint::black_box;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use criterion::{criterion_group, Criterion};
use medulla_agent_types::agent_component::{ModelInputCapabilities, ModelInputMediaType};

/// Counts every allocation the run makes.
struct Counting;

static ALLOCATIONS: AtomicU64 = AtomicU64::new(0);

// SAFETY: forwards to `System` unchanged and only adds a relaxed counter.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        System.alloc(layout)
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        System.dealloc(ptr, layout)
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

const MEDIA: [ModelInputMediaType; 4] = [
    ModelInputMediaType::Text,
    ModelInputMediaType::Png,
    ModelInputMediaType::Jpeg,
    ModelInputMediaType::Webp,
];
const PARTS: u32 = 128;
const MAX_BYTES: u64 = 64 * 1024 * 1024;
const REPORT_SAMPLES: usize = 200;

fn capabilities(media: ModelInputMediaType) -> ModelInputCapabilities {
    ModelInputCapabilities {
        media_types: vec![media],
        max_aggregate_bytes: MAX_BYTES,
        max_parts: PARTS,
    }
}

/// The 128 decoded part sizes of one media type: together exactly the ceiling.
fn parts(media: ModelInputMediaType) -> Vec<(&'static str, u64)> {
    vec![(media.as_str(), MAX_BYTES / u64::from(PARTS)); PARTS as usize]
}

/// Validate every media type's 128 parts through its profile.
fn import_all(profiles: &[(ModelInputCapabilities, Vec<(&'static str, u64)>)]) {
    for (profile, parts) in profiles {
        black_box(profile.validate_parts(black_box(parts))).expect("128 parts import");
    }
}

/// Resident high-water mark in MiB (`VmHWM`), 0 where procfs is absent.
fn high_water_mib() -> f64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| {
            status
                .lines()
                .find(|line| line.starts_with("VmHWM:"))
                .and_then(|line| line.split_whitespace().nth(1))
                .and_then(|kib| kib.parse::<f64>().ok())
        })
        .map_or(0.0, |kib| kib / 1024.0)
}

fn host() -> String {
    std::fs::read_to_string("/etc/hostname")
        .map(|name| name.trim().to_string())
        .unwrap_or_else(|_| "unknown".to_string())
}

fn percentile_ms(sorted: &[Duration], p: f64) -> f64 {
    let idx = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted[idx].as_secs_f64() * 1000.0
}

fn bench_media_facts(c: &mut Criterion) {
    let profiles: Vec<_> = MEDIA
        .iter()
        .map(|media| (capabilities(*media), parts(*media)))
        .collect();
    let mut group = c.benchmark_group("media_facts");
    group.bench_function("four_media_types_at_128_parts", |b| {
        b.iter(|| import_all(&profiles));
    });
    group.finish();

    let before = high_water_mib();
    let allocations_before = ALLOCATIONS.load(Ordering::Relaxed);
    let mut samples: Vec<Duration> = (0..REPORT_SAMPLES)
        .map(|_| {
            let start = Instant::now();
            import_all(&profiles);
            start.elapsed()
        })
        .collect();
    samples.sort();
    let growth = (high_water_mib() - before).max(0.0);
    assert!(
        growth <= 64.0,
        "media metadata grew {growth} MiB, above the 64 MiB ceiling"
    );
    let group_report = serde_json::json!({
        "p50_ms": percentile_ms(&samples, 0.50),
        "p99_ms": percentile_ms(&samples, 0.99),
        "peak_mib": growth,
        "allocations": ALLOCATIONS.load(Ordering::Relaxed) - allocations_before,
        "host": host(),
    });
    if let Ok(path) = std::env::var("DECIDE_BUDGETS_REPORT") {
        let mut report: serde_json::Value = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_else(|| serde_json::json!({ "groups": {} }));
        report["groups"]["media_facts"] = group_report;
        let body = serde_json::to_vec_pretty(&report).unwrap();
        std::fs::write(&path, body).expect("write media facts report");
    }
}

criterion_group!(benches, bench_media_facts);

fn main() {
    benches();
    Criterion::default().configure_from_args().final_summary();
}
