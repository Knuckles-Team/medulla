//! Dung grounded-extension bench (MEDULLA-FEDERATED-QUERY-R700.6).
//!
//! One `DungProjection::grounded` call evaluates 10,000 arguments joined in attack chains of
//! length 4. The gate key `dung_grounded` in `thresholds.json` caps p50 at 1 ms and p99 at
//! 6 ms with a peak memory of 16 MiB.
//!
//! Run: `cargo bench -p medulla-result-types --bench dung_grounded`.
// spec: MEDULLA-FEDERATED-QUERY-R700.6

use criterion::{criterion_group, criterion_main, Criterion};
use medulla_result_types::argument_map::DungProjection;
use std::hint::black_box;

const ARGUMENTS: usize = 10_000;
const CHAIN: usize = 4;

/// Chains `a0 -> a1 -> a2 -> a3`, repeated: the unattacked head of each chain is in the
/// grounded extension, and so is every second argument after it.
fn projection() -> DungProjection {
    let arguments: Vec<String> = (0..ARGUMENTS).map(|n| format!("a{n}")).collect();
    let attacks = (0..ARGUMENTS)
        .filter(|n| n % CHAIN != CHAIN - 1)
        .map(|n| (format!("a{n}"), format!("a{}", n + 1)))
        .collect();
    DungProjection { arguments, attacks }
}

fn bench_grounded(c: &mut Criterion) {
    let projection = projection();
    let warm = projection.grounded();
    // In each chain of four the head and the third argument are accepted.
    assert_eq!(warm.len(), ARGUMENTS / CHAIN * 2);
    c.bench_function("dung_grounded", |b| {
        b.iter(|| black_box(projection.grounded().len()));
    });
}

criterion_group! {
    name = benches;
    config = Criterion::default().sample_size(100);
    targets = bench_grounded
}
criterion_main!(benches);
