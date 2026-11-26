// spec: MEDULLA-SRC-AUTH-R012.10
//! Source sync core at 10,000 records (MEDULLA-SRC-AUTH-R012.10).
//!
//! Nightly bench group `source_sync`: 10,000 synthetic records, a quarter in each
//! of the four outcomes (unchanged, applied, proposed, conflict), go through
//! `reconcile` and the writes through `sync_batch`. The source read is not part of
//! the measurement. Budget per 10,000 records: p50 at most 20 ms, p99 at most 80 ms,
//! peak memory under 64 MiB. When `DECIDE_BUDGETS_REPORT` names a report written by
//! the `decide_budgets` bench, the group is merged into it, so
//! `cargo xtask decide-budgets` reports it against `benches/baselines/decide.json`.
//! Benchmarks never gate a PR.
//!
//! Run: `cargo bench --bench source_sync`

use std::hint::black_box;
use std::time::{Duration, Instant};

use criterion::{criterion_group, Criterion};
use medulla_virtual_graph::source_sync::{
    reconcile, sync_batch, ConflictRule, SyncAction, SyncMutation,
};
use serde_json::{json, Map};

const RECORDS: usize = 10_000;
const REPORT_SAMPLES: usize = 100;
const PEAK_MIB_BUDGET: f64 = 64.0;

/// One record's three digests: the last synced one, the graph's and the source's.
struct Row {
    key: String,
    base: String,
    graph: String,
    source: String,
}

/// A quarter of the records per outcome, in the order of the outcome list.
fn rows() -> Vec<Row> {
    (0..RECORDS)
        .map(|i| {
            let base = format!("sha256:base-{i}");
            let (graph, source) = match i % 4 {
                0 => (base.clone(), base.clone()),
                1 => (base.clone(), format!("sha256:source-{i}")),
                2 => (format!("sha256:graph-{i}"), base.clone()),
                _ => (format!("sha256:graph-{i}"), format!("sha256:source-{i}")),
            };
            Row {
                key: format!("record-{i}"),
                base,
                graph,
                source,
            }
        })
        .collect()
}

/// Reconcile every record and batch the writes of the applied ones with the sync state.
fn sync_all(rows: &[Row]) -> (usize, [usize; 4]) {
    let mut counts = [0usize; 4];
    let mut inbound = Vec::new();
    let mut state = Vec::with_capacity(rows.len());
    for row in rows {
        let action = reconcile(
            Some(&row.base),
            Some(&row.graph),
            Some(&row.source),
            ConflictRule::SystemOfRecordWins,
        );
        let slot = match action {
            SyncAction::Unchanged => 0,
            SyncAction::ApplyInbound => {
                let mut properties = Map::new();
                properties.insert("digest".to_string(), json!(row.source));
                inbound.push(SyncMutation {
                    node_id: row.key.clone(),
                    properties,
                });
                1
            }
            SyncAction::ProposeOutbound => 2,
            SyncAction::Conflict { .. } => 3,
        };
        counts[slot] += 1;
        let mut properties = Map::new();
        properties.insert("last_digest".to_string(), json!(row.source));
        state.push(SyncMutation {
            node_id: format!("state:{}", row.key),
            properties,
        });
    }
    (sync_batch(inbound, state).len(), counts)
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

fn host() -> String {
    std::fs::read_to_string("/etc/hostname")
        .map(|name| name.trim().to_string())
        .unwrap_or_else(|_| "unknown".to_string())
}

fn percentile_ms(sorted: &[Duration], p: f64) -> f64 {
    let idx = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted[idx].as_secs_f64() * 1000.0
}

fn bench_source_sync(c: &mut Criterion) {
    let rows = rows();
    let (written, counts) = sync_all(&rows);
    assert_eq!(counts, [RECORDS / 4; 4], "a quarter of the records per outcome");
    assert_eq!(written, RECORDS / 4 + RECORDS, "applied writes plus one state write each");

    let mut group = c.benchmark_group("source_sync");
    group.bench_function("reconcile_and_batch_10000", |b| {
        b.iter(|| black_box(sync_all(black_box(&rows))));
    });
    group.finish();

    let before = high_water_mib();
    let mut samples: Vec<Duration> = (0..REPORT_SAMPLES)
        .map(|_| {
            let start = Instant::now();
            black_box(sync_all(&rows));
            start.elapsed()
        })
        .collect();
    samples.sort();
    let growth = (high_water_mib() - before).max(0.0);
    assert!(
        growth <= PEAK_MIB_BUDGET,
        "source sync grew {growth} MiB, above the {PEAK_MIB_BUDGET} MiB ceiling"
    );
    let group_report = json!({
        "p50_ms": percentile_ms(&samples, 0.50),
        "p99_ms": percentile_ms(&samples, 0.99),
        "peak_mib": growth,
        "host": host(),
    });
    if let Ok(path) = std::env::var("DECIDE_BUDGETS_REPORT") {
        let mut report: serde_json::Value = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_else(|| json!({ "groups": {} }));
        report["groups"]["source_sync"] = group_report;
        let body = serde_json::to_vec_pretty(&report).unwrap();
        std::fs::write(&path, body).expect("write source sync report");
    }
}

criterion_group!(benches, bench_source_sync);

fn main() {
    benches();
    Criterion::default().configure_from_args().final_summary();
}
