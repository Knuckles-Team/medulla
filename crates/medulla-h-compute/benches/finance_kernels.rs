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
//! Run: `cargo bench -p medulla-h-compute --features finance --bench finance_kernels`.

use criterion::{criterion_group, criterion_main, Criterion};
use medulla_finance::banking::{self, AmlRule};
use medulla_finance_quant::pattern::{classify, PatternKnobs};

const SERIES: usize = 500;
const BARS: usize = 1_000;
const AML_CALLS: i64 = 100_000;

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

criterion_group!(benches, bench_pattern, bench_aml_screen);
criterion_main!(benches);
