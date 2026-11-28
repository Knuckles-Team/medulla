//! Near-duplicate index insert plus query over 100,000 documents
//! (MEDULLA-INGEST-WORKER-R702.6).
//!
//! Nightly bench group `ingest/lsh_index`: each document's signature is queried against the
//! index and then inserted, 100,000 times with the default layout (20 bands of 5 rows).
//! Budget per document: p50 below 1 ms, p99 below 5 ms; peak resident memory (`VmHWM`) below
//! 64 MiB. The run exits non-zero when a budget is exceeded. A service level, not a pull
//! request gate.
//!
// spec: MEDULLA-INGEST-WORKER-R702.6
//!
//! Run: cargo bench -p medulla-compute --bench lsh_index

use std::time::{Duration, Instant};

use {medulla_compute_assemble::sketch::shingles, medulla_compute_assemble::sketch::LshIndex};

const DOCUMENTS: usize = 100_000;
const WORDS: usize = 24;
const THRESHOLD: f64 = 0.8;
const BUDGET_P50: Duration = Duration::from_millis(1);
const BUDGET_P99: Duration = Duration::from_millis(5);
const BUDGET_MIB: f64 = 64.0;

/// A deterministic document of `WORDS` words, distinct per `index`.
fn text(index: usize) -> String {
    let mut state = (index as u64).wrapping_mul(6_364_136_223_846_793_005) | 1;
    let words: Vec<String> = (0..WORDS)
        .map(|_| {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            format!("w{}", state >> 40)
        })
        .collect();
    words.join(" ")
}

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
    let mut index = LshIndex::new(20, 5).expect("the default layout is valid");
    let mut samples = Vec::with_capacity(DOCUMENTS);
    for document in 0..DOCUMENTS {
        let mut sketch = index.new_sketch();
        for shingle in shingles(&text(document)) {
            sketch.insert(&shingle);
        }
        let id = format!("doc-{document}");
        let started = Instant::now();
        let candidates = index.query(&sketch, THRESHOLD);
        index
            .insert(&id, &sketch)
            .expect("signature length matches");
        samples.push(started.elapsed());
        assert!(candidates.is_empty(), "distinct documents never collide");
    }
    samples.sort();
    let p50 = samples[samples.len() / 2];
    let p99 = samples[(samples.len() * 99 / 100).min(samples.len() - 1)];
    let peak = high_water_mib();
    println!(
        "ingest/lsh_index p50 = {p50:?} (budget {BUDGET_P50:?}) p99 = {p99:?} (budget {BUDGET_P99:?}) peak_mib = {peak:.1} (budget {BUDGET_MIB})"
    );
    assert!(p50 < BUDGET_P50, "p50 {p50:?} exceeds {BUDGET_P50:?}");
    assert!(p99 < BUDGET_P99, "p99 {p99:?} exceeds {BUDGET_P99:?}");
    assert!(
        peak < BUDGET_MIB,
        "peak {peak:.1} MiB exceeds {BUDGET_MIB} MiB"
    );
}
