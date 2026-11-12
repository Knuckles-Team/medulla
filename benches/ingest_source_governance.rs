//! Source record governance parsing at batch size (MEDULLA-INGEST-WORKER-R900.6).
//!
//! Nightly bench group `ingest/source_governance`: each batch admits
//! `MAX_SOURCE_RECORDS` (1,024) governed records through
//! `admit_record_governance`, the step `admit_record` runs before it builds a
//! node. Budget per batch: p50 below 2 ms and p99 below 5 ms. The run exits
//! non-zero when a budget is exceeded. A service level, not a pull request gate.
//!
//! spec: MEDULLA-INGEST-WORKER-R900.6
//!
//! Run: cargo bench --bench ingest_source_governance

use std::time::{Duration, Instant};

use medulla_h_ingest::source_ingestion::governance::admit_record_governance;
use medulla_source_types::source_ingestion::MAX_SOURCE_RECORDS;

const BATCHES: usize = 10;
const BUDGET_P50: Duration = Duration::from_millis(2);
const BUDGET_P99: Duration = Duration::from_millis(5);

fn payloads() -> Vec<serde_json::Map<String, serde_json::Value>> {
    (0..MAX_SOURCE_RECORDS)
        .map(|i| {
            serde_json::json!({
                "id": i,
                "name": format!("record-{i}"),
                "_governance": {
                    "classification": "confidential",
                    "access_public": false,
                    "retention": 31_536_000,
                    "legal_hold": i % 7 == 0,
                    "access_read_roles": ["finance", "audit"],
                    "access_markings": ["pii", "sox"],
                }
            })
            .as_object()
            .expect("object payload")
            .clone()
        })
        .collect()
}

fn main() {
    let payloads = payloads();
    let mut samples = Vec::with_capacity(BATCHES);
    for _ in 0..BATCHES {
        let started = Instant::now();
        for payload in &payloads {
            let admitted = admit_record_governance(payload, true).expect("bench records are valid");
            std::hint::black_box(admitted);
        }
        samples.push(started.elapsed());
    }
    samples.sort();
    let p50 = samples[samples.len() / 2];
    let p99 = samples[(samples.len() * 99 / 100).min(samples.len() - 1)];
    println!("ingest/source_governance p50 = {p50:?} (budget {BUDGET_P50:?}) p99 = {p99:?} (budget {BUDGET_P99:?})");
    assert!(p50 < BUDGET_P50, "p50 {p50:?} exceeds {BUDGET_P50:?}");
    assert!(p99 < BUDGET_P99, "p99 {p99:?} exceeds {BUDGET_P99:?}");
}
