//! Catalog search over one million assets (MEDULLA-UNIFIED-DATA-PLANE-R764.6).
//!
//! Nightly bench group `catalog/search_1m` (MEDULLA-DECISION-ENGINE-R178.1): one
//! search ranks 1,000,000 assets with `relevance` (the function the served
//! `search("text")` and `cat:relevance` calls reach) and keeps the first page
//! (`catalog.search_page_max` hits, best score first, ties broken by asset
//! number). Budget per search: p99 at most 30 ms, peak memory growth under 1 GiB.
//! The budgets are asserted as measured; a miss exits non-zero. When
//! `DECIDE_BUDGETS_REPORT` names a report written by the `decide_budgets` bench,
//! the group is merged into it. Benchmarks never gate a PR.
//!
//! Run: `cargo bench --bench catalog_search`

use std::cmp::Ordering;
use std::hint::black_box;
use std::time::{Duration, Instant};

use criterion::{criterion_group, Criterion};
use medulla_catalog_types::catalog_relevance::{
    relevance, AssetDoc, TrustFlag, CATALOG_SEARCH_PAGE_MAX,
};
use serde_json::json;

const ASSETS: usize = 1_000_000;
const REPORT_SAMPLES: usize = 50;
const P99_MS_BUDGET: f64 = 30.0;
const PEAK_MIB_BUDGET: f64 = 1024.0;
const QUERY: &str = "orders";

/// Every asset has a name; every fifth also a description and a glossary term, every
/// hundredth carries a trust flag, and usage varies across the catalog.
fn assets() -> Vec<AssetDoc> {
    (0..ASSETS)
        .map(|i| AssetDoc {
            names: vec![if i % 7 == 0 {
                format!("orders table {i}")
            } else {
                format!("asset {i}")
            }],
            term_labels: if i % 5 == 0 {
                vec!["customer orders".to_string()]
            } else {
                Vec::new()
            },
            concepts: Vec::new(),
            descriptions: if i % 5 == 0 {
                vec![format!("rows of orders for region {}", i % 50)]
            } else {
                Vec::new()
            },
            usage_reads: (i % 200) as u64,
            flags: match i % 100 {
                0 => vec![TrustFlag::Endorsement],
                1 => vec![TrustFlag::Deprecation],
                _ => Vec::new(),
            },
            article_status: None,
            described: Vec::new(),
        })
        .collect()
}

/// One search: score every asset, keep the best page.
fn search(assets: &[AssetDoc], page: usize) -> Vec<(usize, f64)> {
    let mut hits: Vec<(usize, f64)> = assets
        .iter()
        .enumerate()
        .filter_map(|(index, doc)| {
            let score = relevance(doc, QUERY);
            (score > 0.0).then_some((index, score))
        })
        .collect();
    let order = |a: &(usize, f64), b: &(usize, f64)| {
        b.1.partial_cmp(&a.1).unwrap_or(Ordering::Equal).then(a.0.cmp(&b.0))
    };
    if hits.len() > page {
        hits.select_nth_unstable_by(page, order);
        hits.truncate(page);
    }
    hits.sort_by(order);
    hits
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

fn bench_catalog_search(c: &mut Criterion) {
    let before = high_water_mib();
    let assets = assets();
    let page = CATALOG_SEARCH_PAGE_MAX.value() as usize;
    let first = search(&assets, page);
    assert_eq!(first.len(), page, "one full page of hits");
    assert!(
        first.windows(2).all(|pair| pair[0].1 >= pair[1].1),
        "the page is ordered best score first"
    );
    assert_eq!(first, search(&assets, page), "the page is stable across searches");

    let mut group = c.benchmark_group("catalog");
    group.sample_size(10);
    group.bench_function("search_1m", |b| {
        b.iter(|| black_box(search(black_box(&assets), page)));
    });
    group.finish();

    let mut samples: Vec<Duration> = (0..REPORT_SAMPLES)
        .map(|_| {
            let start = Instant::now();
            black_box(search(&assets, page));
            start.elapsed()
        })
        .collect();
    samples.sort();
    let growth = (high_water_mib() - before).max(0.0);
    let p99 = percentile_ms(&samples, 0.99);
    let group_report = json!({
        "p50_ms": percentile_ms(&samples, 0.50),
        "p99_ms": p99,
        "peak_mib": growth,
        "host": host(),
    });
    if let Ok(path) = std::env::var("DECIDE_BUDGETS_REPORT") {
        let mut report: serde_json::Value = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_else(|| json!({ "groups": {} }));
        report["groups"]["catalog/search_1m"] = group_report;
        let body = serde_json::to_vec_pretty(&report).unwrap();
        std::fs::write(&path, body).expect("write catalog search report");
    }
    assert!(
        growth <= PEAK_MIB_BUDGET,
        "catalog search grew {growth} MiB, above the {PEAK_MIB_BUDGET} MiB ceiling"
    );
    assert!(
        p99 <= P99_MS_BUDGET,
        "catalog search p99 {p99} ms is above the {P99_MS_BUDGET} ms budget"
    );
}

criterion_group!(benches, bench_catalog_search);

fn main() {
    benches();
    Criterion::default().configure_from_args().final_summary();
}
