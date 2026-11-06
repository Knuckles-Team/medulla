//! Live-artifact refresh bench (MEDULLA-FEDERATED-QUERY-R701.6).
//!
//! One refresh applies a 1,000-row result by compare-and-set on the artifact version
//! (`LiveArtifact::refresh`). The gate keys `live_artifact_refresh` in `thresholds.json`.
//!
//! Run: `cargo bench -p medulla-plan --bench live_artifact_refresh`.

use criterion::{criterion_group, criterion_main, Criterion};
use medulla_source_types::live_artifact::{
    ArtifactProvenance, LiveArtifact, QueryLanguage, QuerySource,
};
use serde_json::{json, Value};
use std::hint::black_box;

const ROWS: usize = 1_000;

fn artifact() -> LiveArtifact {
    LiveArtifact {
        id: "bench-artifact".into(),
        version: 1,
        template: "Rows: {{0.n}}".into(),
        source: QuerySource {
            language: QueryLanguage::Uql,
            text: "MATCH (:Incident) RETURN count".into(),
        },
        data: Value::Null,
        provenance: ArtifactProvenance::default(),
    }
}

fn rows() -> Value {
    Value::Array((0..ROWS).map(|n| json!({"n": n, "name": "row"})).collect())
}

fn bench_refresh(c: &mut Criterion) {
    let result = rows();
    let mut live = artifact();
    c.bench_function("live_artifact_refresh", |b| {
        b.iter(|| {
            let version = live.version;
            let record = live.refresh(version, Ok(result.clone()), 1, 2);
            black_box(record.expect("version is current"));
        });
    });
}

criterion_group!(benches, bench_refresh);
criterion_main!(benches);
