//! Feedback boost bench (MEDULLA-FEDERATED-QUERY-R705.6).
//!
//! One `apply_boosts` call re-ranks 256 fused candidates by their document boosts, the call
//! `Discover` makes after score calibration. The gate key `feedback_boost_256` in
//! `thresholds.json` caps p99 at 200 microseconds; the p99 gate of `scripts/bench_gate.py`
//! enforces it.
//!
//! Run: `cargo bench -p medulla-plan --bench feedback_boost`.
// spec: MEDULLA-FEDERATED-QUERY-R705.6

use criterion::{criterion_group, criterion_main, Criterion};
use medulla_retrieval::feedback_boost::{apply_boosts, DocumentBoost, DEFAULT_HALF_LIFE_S};
use std::hint::black_box;

const CANDIDATES: usize = 256;

fn candidates() -> Vec<(String, f64)> {
    (0..CANDIDATES)
        .map(|n| (format!("doc-{n}"), 1.0 - n as f64 / CANDIDATES as f64))
        .collect()
}

/// Every fourth candidate carries a boost, the rest none.
fn boosts() -> Vec<DocumentBoost> {
    (0..CANDIDATES)
        .step_by(4)
        .map(|n| DocumentBoost {
            document_ref: format!("doc-{n}"),
            value: if n % 8 == 0 { 0.2 } else { -0.2 },
            half_life_s: DEFAULT_HALF_LIFE_S,
        })
        .collect()
}

fn bench_apply(c: &mut Criterion) {
    let (scores, boosts) = (candidates(), boosts());
    c.bench_function("feedback_boost_256", |b| {
        b.iter(|| {
            let ranked = apply_boosts(black_box(&scores), black_box(&boosts));
            assert_eq!(ranked.len(), CANDIDATES);
            black_box(ranked);
        });
    });
}

criterion_group! {
    name = benches;
    config = Criterion::default().sample_size(100);
    targets = bench_apply
}
criterion_main!(benches);
