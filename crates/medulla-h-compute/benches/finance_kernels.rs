//! Finance kernel bench groups (MEDULLA-FINANCE-PRIMITIVES-R750.753).
//!
//! One file holds every finance kernel group so one thresholds entry covers them. The
//! groups call the pure kernels directly; the served proof is
//! `tests/finance_served_research_kernels.rs`. Budget for `finance/pattern_500x1k`:
//! p50 under 25 ms, p99 under 60 ms, peak memory under 48 MiB, regression above 10
//! percent fails the nightly run.
//!
//! MEDULLA-FINANCE-PRIMITIVES-R901.6: group `finance/aml_screen_100k` loops 100k
//! `banking::screen_transaction` calls per iteration (the served route screens one
//! transaction per call). Budgets in the optimized profile: loop p50 under 20 ms, p99
//! under 50 ms, peak resident growth under 8 MiB; the same 10 percent regression rule
//! applies through the shared thresholds entry.
//!
//! MEDULLA-FINANCE-PRIMITIVES-R900.6: group `finance/shift_100k` runs
//! `sizing::distribution_shift` over two 100k-value samples per iteration. Budgets: p50
//! under 25 ms, p99 under 60 ms, peak resident growth under 8 MiB (`VmHWM`, the probe
//! of MEDULLA-DECISION-ENGINE-R178.1). When `FINANCE_BUDGETS_REPORT` names a path the run
//! writes `p50_ms`, `p99_ms` and `peak_mib` per group, which
//! `scripts/check_decide_budgets.py --baseline benches/baselines/finance.json` compares
//! to the budgets and the stored baseline (a regression above 10 percent fails). The
//! nightly workflow `.github/workflows/nightly-finance-budgets.yml` runs it. It never
//! gates a pull request.
//!
//! Run: `cargo bench -p medulla-h-compute --features finance --bench finance_kernels`.

use criterion::{criterion_group, criterion_main, Criterion};
use medulla_finance::banking::{self, AmlRule};
use medulla_finance_quant::pattern::{classify, PatternKnobs};
use medulla_finance_quant::sizing::distribution_shift;
use std::time::{Duration, Instant};

const SERIES: usize = 500;
const BARS: usize = 1_000;
const AML_CALLS: i64 = 100_000;
const SHIFT_SAMPLES: usize = 100_000;
const REPORT_RUNS: usize = 50;

/// A deterministic pseudo-random walk of `BARS` OHLC bars.
fn series(seed: u64) -> [Vec<f64>; 4] {
    let mut state = seed
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    let mut next = move || {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((state >> 11) as f64) / ((1u64 << 53) as f64)
    };
    let (mut open, mut high, mut low, mut close) = (
        Vec::with_capacity(BARS),
        Vec::with_capacity(BARS),
        Vec::with_capacity(BARS),
        Vec::with_capacity(BARS),
    );
    let mut price = 100.0;
    for _ in 0..BARS {
        let o = price;
        let c = o + (next() - 0.5) * 2.0;
        let h = o.max(c) + next();
        let l = o.min(c) - next();
        open.push(o);
        high.push(h);
        low.push(l);
        close.push(c);
        price = c;
    }
    [open, high, low, close]
}

fn bench_pattern(c: &mut Criterion) {
    let data: Vec<[Vec<f64>; 4]> = (0..SERIES as u64).map(series).collect();
    let knobs = PatternKnobs::default();
    c.bench_function("finance/pattern_500x1k", |b| {
        b.iter(|| {
            let mut total = 0usize;
            for [o, h, l, cl] in &data {
                total += classify(o, h, l, cl, &knobs).len();
            }
            std::hint::black_box(total)
        })
    });
}

fn bench_aml_screen(c: &mut Criterion) {
    let rule = AmlRule {
        rule_id: "ctr".into(),
        version: 1,
        threshold_minor: 10_000,
    };
    c.bench_function("finance/aml_screen_100k", |b| {
        b.iter(|| {
            let mut alerts = 0usize;
            for amount in 1..=AML_CALLS {
                let screened = banking::screen_transaction(
                    std::hint::black_box(&rule),
                    std::hint::black_box(amount % 20_000 + 1),
                );
                alerts += usize::from(screened.expect("valid input").is_some());
            }
            std::hint::black_box(alerts)
        })
    });
}

/// Two 100k-value uniform grids, the second moved by 0.1.
fn shift_samples() -> (Vec<f64>, Vec<f64>) {
    let reference: Vec<f64> = (0..SHIFT_SAMPLES)
        .map(|i| i as f64 / SHIFT_SAMPLES as f64)
        .collect();
    let current = reference.iter().map(|v| v + 0.1).collect();
    (reference, current)
}

fn shift_once(reference: &[f64], current: &[f64]) -> bool {
    distribution_shift(
        std::hint::black_box(reference),
        std::hint::black_box(current),
        0.05,
    )
    .expect("valid samples")
    .shifted
}

// spec: MEDULLA-FINANCE-PRIMITIVES-R900.6
fn bench_shift(c: &mut Criterion) {
    let (reference, current) = shift_samples();
    c.bench_function("finance/shift_100k", |b| {
        b.iter(|| shift_once(&reference, &current))
    });
    if let Ok(path) = std::env::var("FINANCE_BUDGETS_REPORT") {
        let before = high_water_mib();
        let mut runs: Vec<Duration> = (0..REPORT_RUNS)
            .map(|_| {
                let start = Instant::now();
                std::hint::black_box(shift_once(&reference, &current));
                start.elapsed()
            })
            .collect();
        runs.sort();
        let report = serde_json::json!({ "groups": { "finance/shift_100k": {
            "p50_ms": percentile_ms(&runs, 0.50),
            "p99_ms": percentile_ms(&runs, 0.99),
            "peak_mib": (high_water_mib() - before).max(0.0),
        }}});
        std::fs::write(&path, serde_json::to_vec_pretty(&report).unwrap())
            .expect("write finance budget report");
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

fn percentile_ms(sorted: &[Duration], p: f64) -> f64 {
    let idx = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted[idx].as_secs_f64() * 1000.0
}

criterion_group!(benches, bench_pattern, bench_aml_screen, bench_shift);
criterion_main!(benches);
