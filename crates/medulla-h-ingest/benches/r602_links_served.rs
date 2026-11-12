// spec: MEDULLA-REPO-INGEST-R602.6
//! MEDULLA-REPO-INGEST-R602.6 — hot-path proof of the service-link hook.
//!
//! `Method::IndexRepository` → `run_after_commit` → `link_pass` over 50,000
//! routes: the p50 must stay at most 400 ms and the p99 at most 1.2 s.
//!
//! Run: cargo bench -p medulla-h-ingest --bench r602_links_served

use std::hint::black_box;
use std::time::{Duration, Instant};

use criterion::{criterion_group, criterion_main, Criterion};
use medulla_enrich::links::{HttpRoute, ServiceOwnership};
use medulla_enrich::Id;
use medulla_h_ingest::repository_index::enrich::hooks::{link_methods, HookInputs};

const ROUTES: u64 = 50_000;
const SERVICES: u64 = 200;
const SAMPLES: usize = 20;
const P50: Duration = Duration::from_millis(400);
const P99: Duration = Duration::from_millis(1_200);

fn inputs() -> HookInputs {
    let services = (0..SERVICES)
        .map(|s| ServiceOwnership {
            service: Id::new(format!("package:service-{s}")).expect("id"),
            path_prefixes: vec![format!("services/service-{s}/")],
        })
        .collect();
    let routes = (0..ROUTES)
        .map(|n| HttpRoute {
            id: Id::new(format!("http-route:GET:/r{n}")).expect("id"),
            method: "GET".into(),
            path: format!("/r{n}"),
            file: format!("services/service-{}/api/routes-{}.py", n % SERVICES, n % 97),
        })
        .collect();
    HookInputs {
        routes,
        services,
        ..HookInputs::default()
    }
}

fn percentile(sorted: &[Duration], quantile: f64) -> Duration {
    let last = sorted.len() - 1;
    sorted[((last as f64) * quantile).ceil() as usize]
}

fn links_served(c: &mut Criterion) {
    let inputs = inputs();
    let mut samples: Vec<Duration> = (0..SAMPLES)
        .map(|_| {
            let started = Instant::now();
            black_box(link_methods(black_box(&inputs)).expect("the link hook runs"));
            started.elapsed()
        })
        .collect();
    samples.sort();
    assert!(percentile(&samples, 0.5) <= P50, "p50 {samples:?}");
    assert!(percentile(&samples, 0.99) <= P99, "p99 {samples:?}");
    let mut group = c.benchmark_group("repo_index/links");
    group.bench_function("links_served", |b| {
        b.iter(|| link_methods(black_box(&inputs)))
    });
    group.finish();
}

criterion_group!(benches, links_served);
criterion_main!(benches);
