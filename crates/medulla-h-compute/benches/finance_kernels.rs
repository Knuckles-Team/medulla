//! Finance kernel bench groups (MEDULLA-FINANCE-PRIMITIVES-R750.753).
//!
//! One file holds every finance kernel group so one thresholds entry covers them. The
//! groups call the pure kernels directly; the served proof is
//! `tests/finance_served_research_kernels.rs`. Budget for `finance/pattern_500x1k`:
//! p50 under 25 ms, p99 under 60 ms, peak memory under 48 MiB, regression above 10
//! percent fails the nightly run.
//!
//! Run: `cargo bench -p medulla-h-compute --features finance --bench finance_kernels`.

use criterion::{criterion_group, criterion_main, Criterion};
use medulla_finance_quant::pattern::{classify, PatternKnobs};

const SERIES: usize = 500;
const BARS: usize = 1_000;

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

criterion_group!(benches, bench_pattern);
criterion_main!(benches);
