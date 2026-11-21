//! Standards drift scoring per request (MEDULLA-UNIFIED-DATA-PLANE-R701.6).
//!
//! Nightly bench group `standards_drift`: one request's scoring pass over 2,000
//! assets in 20 capability classes (`drift_score`, `form_cohorts` and
//! `recommend_consolidation`), the pure core `handle_standards_drift` runs after it
//! reads the nodes. Budget per request: p50 at most 4 ms, p99 at most 30 ms, peak
//! memory under 64 MiB (MEDULLA-DECISION-ENGINE-R178). When `DECIDE_BUDGETS_REPORT`
//! names a report written by the `decide_budgets` bench, the group is merged into
//! it. Benchmarks never gate a PR.
//!
//! Run: `cargo bench --bench standards_drift`

use std::collections::BTreeSet;
use std::hint::black_box;
use std::time::{Duration, Instant};

use criterion::{criterion_group, Criterion};
use medulla_source_types::standards_drift::{
    drift_score, form_cohorts, recommend_consolidation, Standard, StandardRule, StandardsAsset,
};
use serde_json::json;

const ASSETS: usize = 2_000;
const CLASSES: usize = 20;
const REPORT_SAMPLES: usize = 100;
const PEAK_MIB_BUDGET: f64 = 64.0;

fn standards() -> Vec<Standard> {
    (0..CLASSES)
        .map(|class| Standard {
            id: format!("standard-{class}"),
            capability_class: format!("class-{class}"),
            rules: (0..4)
                .map(|rule| StandardRule {
                    id: format!("rule-{rule}"),
                    required_fact: format!("fact-{rule}"),
                    weight: 1.0 + rule as f64,
                })
                .collect(),
        })
        .collect()
}

/// Every tenth asset belongs to a class with no standard.
fn assets() -> Vec<StandardsAsset> {
    (0..ASSETS)
        .map(|i| {
            let class = if i % 10 == 9 {
                CLASSES + 1
            } else {
                i % CLASSES
            };
            StandardsAsset {
                id: format!("asset-{i}"),
                tenant: "bench-tenant".to_string(),
                capability_classes: BTreeSet::from([format!("class-{class}")]),
                facts: (0..4 + i % 2)
                    .filter(|fact| (i + fact) % 5 != 0)
                    .map(|fact| format!("fact-{fact}"))
                    .collect(),
                cost: 10 + (i % 7) as u64,
            }
        })
        .collect()
}

/// One request: score every asset, form the cohorts, recommend for each cohort.
fn request(standards: &[Standard], assets: &[StandardsAsset]) -> (usize, usize) {
    let scores: Vec<_> = assets
        .iter()
        .map(|asset| drift_score(asset, standards))
        .collect();
    let cohorts = form_cohorts(assets);
    let mut recommendations = 0;
    for cohort in &cohorts {
        let members: Vec<&StandardsAsset> = assets
            .iter()
            .filter(|asset| cohort.assets.contains(&asset.id))
            .collect();
        if recommend_consolidation(&cohort.capability_class, &members).is_ok() {
            recommendations += 1;
        }
    }
    (scores.len(), recommendations)
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

fn bench_standards_drift(c: &mut Criterion) {
    let standards = standards();
    let assets = assets();
    let (scored, recommended) = request(&standards, &assets);
    assert_eq!(scored, ASSETS, "every asset gets a score entry");
    assert!(recommended > 0, "the cohorts yield recommendations");

    let mut group = c.benchmark_group("standards_drift");
    group.bench_function("score_cohort_recommend_2000", |b| {
        b.iter(|| black_box(request(black_box(&standards), black_box(&assets))));
    });
    group.finish();

    let before = high_water_mib();
    let mut samples: Vec<Duration> = (0..REPORT_SAMPLES)
        .map(|_| {
            let start = Instant::now();
            black_box(request(&standards, &assets));
            start.elapsed()
        })
        .collect();
    samples.sort();
    let growth = (high_water_mib() - before).max(0.0);
    assert!(
        growth <= PEAK_MIB_BUDGET,
        "standards drift grew {growth} MiB, above the {PEAK_MIB_BUDGET} MiB ceiling"
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
        report["groups"]["standards_drift"] = group_report;
        let body = serde_json::to_vec_pretty(&report).unwrap();
        std::fs::write(&path, body).expect("write standards drift report");
    }
}

criterion_group!(benches, bench_standards_drift);

fn main() {
    benches();
    Criterion::default().configure_from_args().final_summary();
}
