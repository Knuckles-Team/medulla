//! Evidence-forest selection bench (MEDULLA-FEDERATED-QUERY-R736.5).
//!
//! `select_forest` over 2,000 candidates under a 4,000-token budget: the R736.4 latency
//! budget (p99 150 ms). The criterion id `retrieval/evidence_forest` is the key in
//! `thresholds.json` (`latency_p99_ns_max`). Nightly only; never gates a pull request.
//!
//! Run: `cargo bench -p medulla-plan --features query --bench evidence_forest`.
// spec: MEDULLA-FEDERATED-QUERY-R736.5

use criterion::{criterion_group, criterion_main, Criterion};
use medulla_plan::evidence_forest::{select_forest, ForestPenalty, ForestRelations};
use medulla_result_types::evidence_forest::EvidenceCandidate;
use std::hint::black_box;

const CANDIDATES: usize = 2_000;
const BUDGET_TOKENS: u32 = 4_000;

/// Every pair of memories is mildly similar and none conflicts.
struct Flat;

impl ForestRelations for Flat {
    fn similarity(&self, _a: &str, _b: &str) -> f32 {
        0.1
    }
    fn conflicts(&self, _a: &str, _b: &str) -> bool {
        false
    }
}

fn candidates() -> Vec<EvidenceCandidate> {
    (0..CANDIDATES)
        .map(|n| EvidenceCandidate {
            memory_id: format!("memory-{n:05}"),
            parent: "seed-0".to_string(),
            support: 0.5 + (n % 50) as f32 / 100.0,
            verify_cost: 0.1,
            tokens: 8 + (n % 9) as u32,
        })
        .collect()
}

fn bench_forest(c: &mut Criterion) {
    let pool = candidates();
    let seeds = vec!["seed-0".to_string()];
    let mut group = c.benchmark_group("retrieval");
    group.bench_function("evidence_forest", |b| {
        b.iter(|| {
            let forest = select_forest(
                black_box(&pool),
                &seeds,
                &Flat,
                BUDGET_TOKENS,
                ForestPenalty::default(),
            )
            .expect("the pool selects");
            black_box(forest.tokens_used);
        });
    });
    group.finish();
}

criterion_group!(benches, bench_forest);
criterion_main!(benches);
