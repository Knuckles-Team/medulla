// spec: MEDULLA-REPO-INGEST-R603.6
//! MEDULLA-REPO-INGEST-R603.6 — hot-path proof of structural fingerprints.
//!
//! `Method::IndexRepository` → `run_after_commit` → `fingerprint` over 10,000
//! files: the p50 must stay at most 300 ms and the p99 at most 900 ms. The
//! blast radius of one symbol over 100,000 files must stay at most 150 ms.
//!
//! Run: cargo bench -p medulla-h-ingest --bench r603_fingerprint_served

use std::collections::{BTreeMap, BTreeSet};
use std::hint::black_box;
use std::time::{Duration, Instant};

use criterion::{criterion_group, criterion_main, Criterion};
use medulla_enrich::fingerprint::{blast_radius, fingerprint};
use medulla_enrich::Id;

const FINGERPRINT_FILES: usize = 10_000;
const BLAST_FILES: usize = 100_000;
const SAMPLES: usize = 20;
const P50: Duration = Duration::from_millis(300);
const P99: Duration = Duration::from_millis(900);
const BLAST_MAX: Duration = Duration::from_millis(150);

struct SourceFile {
    path: Id,
    content: Vec<u8>,
    signatures: Vec<String>,
}

fn source_files() -> Vec<SourceFile> {
    (0..FINGERPRINT_FILES)
        .map(|n| SourceFile {
            path: Id::new(format!("file:src/module_{}/file_{n}.rs", n % 100)).expect("id"),
            content: format!("pub fn handler_{n}() {{}}\n{}", "// body line\n".repeat(40))
                .into_bytes(),
            signatures: (0..8)
                .map(|s| format!("fn handler_{n}_{s}(a: u64) -> u64"))
                .collect(),
        })
        .collect()
}

fn percentile(sorted: &[Duration], quantile: f64) -> Duration {
    let last = sorted.len() - 1;
    sorted[((last as f64) * quantile).ceil() as usize]
}

fn fingerprint_all(files: &[SourceFile]) {
    for file in files {
        black_box(fingerprint(
            file.path.clone(),
            black_box(&file.content),
            Ok(file.signatures.as_slice()),
        ));
    }
}

fn blast_corpus() -> BTreeMap<String, Vec<String>> {
    (0..BLAST_FILES)
        .map(|n| {
            let line = if n % 50 == 0 {
                "let value = target_symbol(input);".to_string()
            } else {
                format!("let value_{n} = other_call({n});")
            };
            (format!("src/file_{n}.rs"), vec![line])
        })
        .collect()
}

fn fingerprint_served(c: &mut Criterion) {
    let files = source_files();
    let mut samples: Vec<Duration> = (0..SAMPLES)
        .map(|_| {
            let started = Instant::now();
            fingerprint_all(&files);
            started.elapsed()
        })
        .collect();
    samples.sort();
    assert!(percentile(&samples, 0.5) <= P50, "p50 {samples:?}");
    assert!(percentile(&samples, 0.99) <= P99, "p99 {samples:?}");

    let corpus = blast_corpus();
    let symbol = Id::new("symbol:target_symbol").expect("id");
    let definitions: BTreeSet<(String, usize)> = BTreeSet::new();
    let started = Instant::now();
    let radius = black_box(blast_radius(
        &symbol,
        "target_symbol",
        &corpus,
        &definitions,
        3,
    ));
    let elapsed = started.elapsed();
    assert_eq!(radius.usages, (BLAST_FILES / 50) as u64);
    assert!(elapsed <= BLAST_MAX, "blast radius took {elapsed:?}");

    let mut group = c.benchmark_group("repo_index/fingerprint");
    group.bench_function("fingerprint_served", |b| b.iter(|| fingerprint_all(&files)));
    group.finish();
}

criterion_group!(benches, fingerprint_served);
criterion_main!(benches);
