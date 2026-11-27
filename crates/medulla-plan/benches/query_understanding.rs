//! Query-understanding bench (MEDULLA-FEDERATED-QUERY-R651.6).
//!
//! One `understand` call analyses a 20-token query: its `key:value` filters, its phrases and
//! its relational intent against the caller graph's relationship vocabulary. The served
//! `Discover` mode `deep` calls exactly this function from `plan_discover`. The gate key
//! `query_understanding_plan` in `thresholds.json` caps p50 at 300 microseconds and p99 at
//! 2 ms.
//!
//! Run: `cargo bench -p medulla-plan --bench query_understanding`.
// spec: MEDULLA-FEDERATED-QUERY-R651.6

use criterion::{criterion_group, criterion_main, Criterion};
use medulla_plan::query_understanding::{understand, RelationshipName};
use std::hint::black_box;

/// Twenty whitespace-separated tokens: two filters, a quoted phrase and a relational clause.
const QUERY: &str =
    "type:Doc source:wiki after:2026-01-01 \"replica recovery\" which team owns the \
                     replica notes that describe wal replay and snapshot restore for every cluster";

fn relationships() -> Vec<RelationshipName> {
    [
        "owns",
        "depends_on",
        "describes",
        "supersedes",
        "cites",
        "reviews",
    ]
    .into_iter()
    .map(|name| RelationshipName::new(name.to_string(), None))
    .collect()
}

fn bench_understand(c: &mut Criterion) {
    let relationships = relationships();
    c.bench_function("query_understanding_plan", |b| {
        b.iter(|| {
            let analysis = understand(black_box(QUERY), black_box(&relationships));
            black_box(analysis.terms.len());
        });
    });
}

criterion_group!(benches, bench_understand);
criterion_main!(benches);
