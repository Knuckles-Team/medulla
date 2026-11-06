//! MEDULLA-REPO-INGEST-R601.6 — hot-path proof of the history hook.
//!
//! `Method::IndexRepository` → `run_after_commit` → `ingest_history` over
//! 100,000 commits: the p50 must stay at most 3 s and the p99 at most 8 s.
//! The coupling pass counts at most `max_files_per_commit` squared pairs per
//! commit.
//!
//! Run: cargo bench -p medulla-h-ingest --bench r601_history_served

use std::collections::BTreeSet;
use std::hint::black_box;
use std::time::{Duration, Instant};

use criterion::{criterion_group, criterion_main, Criterion};
use medulla_enrich::history::{ChangeKind, CommitRecord, FileChange, HistoryConfig};
use medulla_enrich::Id;
use medulla_h_ingest::repository_index::enrich::hooks::history_methods;

const COMMITS: u64 = 100_000;
const FILES: u64 = 2_000;
const FILES_PER_COMMIT: u64 = 4;
const SAMPLES: usize = 10;
const P50: Duration = Duration::from_secs(3);
const P99: Duration = Duration::from_secs(8);

fn commit(n: u64) -> CommitRecord {
    let changes = (0..FILES_PER_COMMIT)
        .map(|k| FileChange {
            path: Id::new(format!("src/file-{}.rs", (n * 7 + k * 13) % FILES)).expect("id"),
            kind: ChangeKind::Modified,
            added_lines: 3,
            removed_lines: 1,
        })
        .collect();
    CommitRecord::new(
        &format!("{n:040x}"),
        Id::new(format!("author-{}", n % 50)).expect("id"),
        n,
        Vec::new(),
        changes,
    )
    .expect("a valid commit")
}

fn percentile(sorted: &[Duration], quantile: f64) -> Duration {
    let last = sorted.len() - 1;
    sorted[((last as f64) * quantile).ceil() as usize]
}

fn history_served(c: &mut Criterion) {
    let commits: Vec<CommitRecord> = (1..=COMMITS).map(commit).collect();
    let config = HistoryConfig::default();
    let mut samples: Vec<Duration> = (0..SAMPLES)
        .map(|_| {
            let started = Instant::now();
            let out = history_methods(black_box(&commits), BTreeSet::new(), &config);
            black_box(out.expect("the history hook runs"));
            started.elapsed()
        })
        .collect();
    samples.sort();
    assert!(percentile(&samples, 0.5) <= P50, "p50 {samples:?}");
    assert!(percentile(&samples, 0.99) <= P99, "p99 {samples:?}");
    let mut group = c.benchmark_group("repo_index/history");
    group.sample_size(10);
    group.bench_function("history_served", |b| {
        b.iter(|| history_methods(black_box(&commits), BTreeSet::new(), &config))
    });
    group.finish();
}

criterion_group!(benches, history_served);
criterion_main!(benches);
