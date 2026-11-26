//! Glossary label search over 100,000 terms (MEDULLA-TYPED-PACKS-R801.6).
//!
//! Nightly bench group `glossary_search`: 100,000 terms, each with a preferred label and two
//! alternative labels, searched by one label query through `glossary::label_matches`.
//! Budget: p99 under 5 ms per search. When `DECIDE_BUDGETS_REPORT` names a report, the group is
//! merged into it. Benchmarks never gate a PR.
//!
//! Run: `cargo bench --bench glossary_search`

use std::hint::black_box;
use std::time::{Duration, Instant};

use criterion::{criterion_group, Criterion};
use medulla_catalog_types::glossary::{GlossaryTerm, GlossaryTermStatus};
use medulla_core::glossary::label_matches;
use serde_json::json;

const TERMS: usize = 100_000;
const REPORT_SAMPLES: usize = 100;
const P99_BUDGET_MS: f64 = 5.0;

fn terms() -> Vec<GlossaryTerm> {
    (0..TERMS)
        .map(|i| GlossaryTerm {
            scheme: "urn:scheme:bench".to_string(),
            pref_label: format!("Measure {i} revenue"),
            alt_labels: vec![format!("Turnover {i}"), format!("Sales figure {i}")],
            definition: "Income from sales.".to_string(),
            broader: Vec::new(),
            related: Vec::new(),
            status: GlossaryTermStatus::Approved,
            replaced_by: None,
        })
        .collect()
}

/// One search: how many terms match the label query.
fn search(terms: &[GlossaryTerm], query: &str) -> usize {
    terms
        .iter()
        .filter(|term| label_matches(term, query))
        .count()
}

fn percentile_ms(sorted: &[Duration], quantile: f64) -> f64 {
    let index = ((sorted.len() as f64 * quantile).ceil() as usize).saturating_sub(1);
    sorted[index.min(sorted.len() - 1)].as_secs_f64() * 1000.0
}

fn bench_glossary_search(c: &mut Criterion) {
    let terms = terms();
    // An alternative label finds exactly the one term that carries it.
    assert_eq!(search(&terms, "turnover 4242"), 1);

    let mut group = c.benchmark_group("glossary_search");
    group.bench_function("label_search_100000", |b| {
        b.iter(|| black_box(search(black_box(&terms), "turnover 4242")));
    });
    group.finish();

    let mut samples: Vec<Duration> = (0..REPORT_SAMPLES)
        .map(|_| {
            let start = Instant::now();
            black_box(search(&terms, "turnover 4242"));
            start.elapsed()
        })
        .collect();
    samples.sort();
    let p99 = percentile_ms(&samples, 0.99);
    let group_report = json!({
        "p50_ms": percentile_ms(&samples, 0.50),
        "p99_ms": p99,
        "budget_p99_ms": P99_BUDGET_MS,
        "within_budget": p99 <= P99_BUDGET_MS,
    });
    if let Ok(path) = std::env::var("DECIDE_BUDGETS_REPORT") {
        let mut report: serde_json::Value = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_else(|| json!({ "groups": {} }));
        report["groups"]["glossary_search"] = group_report;
        let body = serde_json::to_vec_pretty(&report).unwrap();
        std::fs::write(&path, body).expect("write glossary search report");
    }
}

criterion_group!(benches, bench_glossary_search);

fn main() {
    benches();
    Criterion::default().configure_from_args().final_summary();
}
