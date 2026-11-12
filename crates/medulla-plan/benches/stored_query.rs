//! Stored-query read bench (MEDULLA-FEDERATED-QUERY-R708.6).
//!
//! One call reads a materialized result of 10,000 rows inside its period. The served
//! `StoredQueryCall` handler answers it with `serve_materialized`, which returns the stored
//! record without running the query; this measures that read. The gate key `stored_query` in
//! `thresholds.json` caps p99 at 3 ms.
//!
//! Run: `cargo bench -p medulla-plan --bench stored_query`.

use criterion::{criterion_group, criterion_main, Criterion};
use medulla_source_types::stored_query::{serve_materialized, RefreshPolicy, StoredResult};
use serde_json::{json, Value};
use std::hint::black_box;

const ROWS: usize = 10_000;

fn stored() -> StoredResult {
    StoredResult {
        columns: vec!["n".into()],
        rows: Value::Array((0..ROWS).map(|n| json!({"id": n, "n": n})).collect()),
        row_count: ROWS as u64,
        input_generation: 7,
        refreshed_at_ms: 1_000,
    }
}

fn bench_read(c: &mut Criterion) {
    let policy = RefreshPolicy::Period { seconds: 3_600 };
    let record = stored();
    c.bench_function("stored_query", |b| {
        b.iter(|| {
            let (result, served) = serve_materialized(policy, Some(record.clone()), 2_000, || {
                Err("a current result never runs the query".to_string())
            })
            .expect("the stored result is current");
            black_box((result.row_count, served));
        });
    });
}

criterion_group!(benches, bench_read);
criterion_main!(benches);
