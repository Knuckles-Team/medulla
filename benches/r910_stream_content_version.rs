//! Stream content-version chaining budget (MEDULLA-INGEST-WORKER-R910.6).
//!
//! Workload: a 10,000-record `SourceIngest` batch. Each sample digests the
//! batch, then builds the content version that chains from the prior batch's
//! digest of the same stream, as `prepare` does for every batch. Budgets: p99
//! at most 20 ms and peak resident memory (`VmHWM`) at most 64 MiB. A bench
//! never gates a PR; the nightly run reports it.
//!
//! Run: `cargo bench --bench r910_stream_content_version`

use std::time::{Duration, Instant};

use criterion::{criterion_group, criterion_main, Criterion};
use medulla_protocol_types::change_envelope::{ContentVersion, ContentVersionPosition};
use medulla_source_types::source_ingestion::{
    fixtures, SourceIngestionRequest, SourceJson, SourceRecord,
};
use medulla_types::contract::BoundedVec;

const RECORDS: usize = 10_000;
const SAMPLES: usize = 50;
const P99_BUDGET: Duration = Duration::from_millis(20);
const PEAK_BUDGET_MIB: f64 = 64.0;

fn batch(page: u64) -> SourceIngestionRequest {
    let mut batch = fixtures::request().as_batch().clone();
    let template = batch.records.as_slice()[0].clone();
    let records: Vec<SourceRecord> = (0..RECORDS)
        .map(|n| {
            let mut record = template.clone();
            record.record_id = format!("item-{n}");
            record.payload = SourceJson::new(serde_json::json!({"id": n, "page": page}))
                .expect("bounded source JSON");
            record
        })
        .collect();
    batch.records = BoundedVec::new(records).expect("bounded source records");
    batch.provider_checkpoint.position =
        SourceJson::new(serde_json::json!({"page": page})).expect("bounded cursor JSON");
    SourceIngestionRequest::new(batch).expect("valid source ingestion request")
}

/// Resident high-water mark in MiB (`VmHWM`), 0 where procfs is absent.
fn high_water_mib() -> f64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| {
            status
                .lines()
                .find(|line| line.starts_with("VmHWM:"))
                .and_then(|line| line.split_whitespace().nth(1))
                .and_then(|kib| kib.parse::<f64>().ok())
        })
        .map_or(0.0, |kib| kib / 1024.0)
}

fn chained_version(request: &SourceIngestionRequest, previous: Option<String>) -> ContentVersion {
    let digest = request.batch_digest().expect("batch digest").to_hex();
    ContentVersion {
        object_id: "source-live-set:bench".into(),
        digest_algorithm: "sha256".into(),
        digest,
        previous_digest: previous,
        source_version: ContentVersionPosition::Opaque {
            version_type: "source-cursor-sha256".into(),
            value: "00".repeat(32),
        },
    }
}

// spec: MEDULLA-INGEST-WORKER-R910.6
fn r910_stream_content_version(c: &mut Criterion) {
    let requests: Vec<_> = (0..SAMPLES as u64).map(|page| batch(page + 2)).collect();
    let mut latencies = Vec::with_capacity(SAMPLES);
    let mut previous = None;
    for request in &requests {
        let started = Instant::now();
        let version = chained_version(request, previous.take());
        latencies.push(started.elapsed());
        previous = Some(version.digest);
    }
    latencies.sort();
    let p99 = latencies[((latencies.len() as f64 - 1.0) * 0.99).round() as usize];
    let peak = high_water_mib();
    eprintln!("ingest/stream_content_version p99={p99:?} peak_mib={peak:.1}");
    assert!(p99 <= P99_BUDGET, "p99 {p99:?} exceeds {P99_BUDGET:?}");
    assert!(
        peak <= PEAK_BUDGET_MIB,
        "peak {peak:.1} MiB exceeds {PEAK_BUDGET_MIB} MiB"
    );

    let request = &requests[0];
    c.bench_function("ingest/stream_content_version", |b| {
        b.iter(|| chained_version(request, Some("00".repeat(32))))
    });
}

criterion_group!(benches, r910_stream_content_version);
criterion_main!(benches);
