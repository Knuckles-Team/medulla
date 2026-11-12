//! MEDULLA-SDLC-R002.6 — hot-path budget of the served deployment record.
//!
//! `Method::RecordDeployment` → `dispatch_sdlc_methods` →
//! `handle_record_deployment`: each record resolves its release by image digest
//! over 50 releases. The p99 per record must stay at most 2 ms. The benchmark
//! gates only the nightly run, never a pull request.
//!
//! Run: cargo bench -p medulla-h-ingest --bench lifecycle_record_deployment

use std::hint::black_box;
use std::time::{Duration, Instant};

use criterion::{criterion_group, criterion_main, Criterion};
use medulla_source_types::lifecycle::{
    handle_record_deployment, DeploymentInput, DeploymentOutcome, Release,
};

const RELEASES: u64 = 50;
const RECORDS: u64 = 1_000;
const P99: Duration = Duration::from_millis(2);

fn releases() -> Vec<Release> {
    (0..RELEASES)
        .map(|n| Release {
            repo: "medulla".into(),
            version: format!("1.0.{n}"),
            git_ref: format!("refs/tags/v1.0.{n}"),
            changeset_head: format!("{n:040x}"),
            artifact_digests: vec![format!("sha256:{n:064x}")],
            published_at: n,
        })
        .collect()
}

fn input(n: u64) -> DeploymentInput {
    DeploymentInput {
        environment: "prod".into(),
        workload: format!("engine-{}", n % 7),
        image_digest: format!("sha256:{:064x}", n % RELEASES),
        started_at: n,
        finished_at: n + 1,
        outcome: DeploymentOutcome::Succeeded,
    }
}

fn record_deployment(c: &mut Criterion) {
    let releases = releases();
    let mut samples: Vec<Duration> = (0..RECORDS)
        .map(|n| {
            let started = Instant::now();
            black_box(handle_record_deployment(
                black_box(&releases),
                black_box(input(n)),
            ));
            started.elapsed()
        })
        .collect();
    samples.sort();
    let p99 = samples[((samples.len() - 1) as f64 * 0.99).ceil() as usize];
    assert!(p99 <= P99, "p99 {p99:?} over the 2 ms budget");
    let mut group = c.benchmark_group("lifecycle/record_deployment");
    group.bench_function("record_deployment", |b| {
        let mut n = 0;
        b.iter(|| {
            n += 1;
            handle_record_deployment(black_box(&releases), black_box(input(n)))
        })
    });
    group.finish();
}

criterion_group!(benches, record_deployment);
criterion_main!(benches);
