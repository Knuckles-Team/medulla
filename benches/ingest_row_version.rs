//! Row-version admission at batch size (MEDULLA-INGEST-WORKER-R901.6).
//!
//! Nightly bench group `ingest/row_version`: `admit_row_version` over 10 batches of 1,024 rows,
//! half first versions and half newer versions of a held row. Budget per batch: p50 below 6 ms,
//! p99 below 15 ms; peak resident memory (`VmHWM`) below 32 MiB. The run exits non-zero when a
//! budget is exceeded. A service level, not a pull request gate.
//!
// spec: MEDULLA-INGEST-WORKER-R901.6
//!
//! Run: cargo bench --bench ingest_row_version

use std::time::{Duration, Instant};

use medulla_types::contract::Digest256;
use {
    medulla_h_ingest_repository::source_ingestion::row_version::admit_row_version,
    medulla_h_ingest_repository::source_ingestion::row_version::HeldRowVersion,
    medulla_h_ingest_repository::source_ingestion::row_version::RowVersionIds,
    medulla_h_ingest_repository::source_ingestion::row_version::RowVersionMember,
};

const BATCHES: usize = 10;
const ROWS: usize = 1_024;
const BUDGET_P50: Duration = Duration::from_millis(6);
const BUDGET_P99: Duration = Duration::from_millis(15);
const BUDGET_MIB: f64 = 32.0;

/// Resident high-water mark in MiB (`VmHWM`), 0 where procfs is absent.
fn high_water_mib() -> f64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| {
            status
                .lines()
                .find(|line| line.starts_with("VmHWM:"))
                .and_then(|line| line.split_whitespace().nth(1)?.parse::<f64>().ok())
        })
        .map_or(0.0, |kib| kib / 1024.0)
}

fn main() {
    let node_ids: Vec<String> = (0..ROWS)
        .map(|row| {
            format!(
                "source:{}",
                Digest256::sha256(format!("row-{row}").as_bytes()).to_hex()
            )
        })
        .collect();
    let digest = Digest256::sha256(b"evidence").to_hex();
    let held = HeldRowVersion {
        version: 1,
        evidence_digest: digest.clone(),
    };
    let mut samples = Vec::with_capacity(BATCHES);
    for batch in 0..BATCHES {
        let started = Instant::now();
        for (row, node_id) in node_ids.iter().enumerate() {
            let member = RowVersionMember {
                table: "orders".into(),
                version: serde_json::json!(2 + batch),
            };
            let ids = RowVersionIds {
                tenant: "tenant:bench",
                connector: "db",
                stream: "orders",
                node_id,
            };
            let held = (row % 2 == 1).then_some(&held);
            admit_row_version(held, &member, &digest, &ids).expect("bench versions are valid");
        }
        samples.push(started.elapsed());
    }
    samples.sort();
    let p50 = samples[samples.len() / 2];
    let p99 = samples[(samples.len() * 99 / 100).min(samples.len() - 1)];
    let peak = high_water_mib();
    println!(
        "ingest/row_version p50 = {p50:?} (budget {BUDGET_P50:?}) p99 = {p99:?} (budget {BUDGET_P99:?}) peak_mib = {peak:.1} (budget {BUDGET_MIB})"
    );
    assert!(p50 < BUDGET_P50, "p50 {p50:?} exceeds {BUDGET_P50:?}");
    assert!(p99 < BUDGET_P99, "p99 {p99:?} exceeds {BUDGET_P99:?}");
    assert!(
        peak < BUDGET_MIB,
        "peak {peak:.1} MiB exceeds {BUDGET_MIB} MiB"
    );
}
