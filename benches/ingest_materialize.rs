//! Materialization at batch size (MEDULLA-INGEST-WORKER-R606.6).
//!
//! Nightly bench group `ingest/materialize`: 100 batches of 1,024 records, the batch bound
//! `MAX_SOURCE_RECORDS`. Each record takes the per-record materialization the served
//! `SourceIngest` runs inside `prepare`: the class label normalization and the three
//! provenance stamps of `stamp_record`. Budget per batch: p50 under 120 ms and p99 under
//! 300 ms. A service level, not a pull request gate. The served end-to-end proof is
//! `tests/ingest/test_materialize_served.py`.
//!
// spec: MEDULLA-INGEST-WORKER-R606.6
//!
//! Run: cargo bench --bench ingest_materialize

use std::time::{Duration, Instant};

use medulla_h_ingest::source_ingestion::{normalize_label, stamp_record};
use medulla_types::ingest_materialize::MaterializeStamp;

const BATCHES: usize = 100;
const RECORDS: usize = 1_024;
const BUDGET_P50: Duration = Duration::from_millis(120);
const BUDGET_P99: Duration = Duration::from_millis(300);

fn main() {
    let classes = [
        "Document",
        "https://knuckles-team.github.io/ontology/core#Person",
        ":Ticket",
        "Caf\u{e9}-Order",
    ];
    let mut samples = Vec::with_capacity(BATCHES);
    for batch in 0..BATCHES {
        let started = Instant::now();
        let mut written = 0usize;
        for index in 0..RECORDS {
            let mut properties = serde_json::Map::new();
            properties.insert(
                "type".into(),
                normalize_label(classes[index % classes.len()]).into(),
            );
            properties.insert("name".into(), format!("record {batch}-{index}").into());
            stamp_record(
                &mut properties,
                &MaterializeStamp {
                    source_system: "bench-connector".to_string(),
                    source_id: format!("record-{batch}-{index}"),
                    ingest_time_ms: 1_700_000_000_000 + batch as u64,
                },
            );
            written += properties.len();
        }
        std::hint::black_box(written);
        samples.push(started.elapsed());
    }
    samples.sort();
    let p50 = samples[samples.len() / 2];
    let p99 = samples[(samples.len() * 99 / 100).min(samples.len() - 1)];
    println!(
        "ingest/materialize p50 = {p50:?} (budget {BUDGET_P50:?}), p99 = {p99:?} (budget {BUDGET_P99:?}) per batch of {RECORDS}"
    );
    assert!(p50 < BUDGET_P50, "p50 {p50:?} exceeds {BUDGET_P50:?}");
    assert!(p99 < BUDGET_P99, "p99 {p99:?} exceeds {BUDGET_P99:?}");
}
