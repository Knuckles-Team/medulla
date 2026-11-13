//! Ontology diff hot-path proof (MEDULLA-TYPED-PACKS-R804.6).
//!
//! `diff_axioms` over two 5,000-axiom ontology revisions: the second revision
//! drops 250 classes, adds 250 and rewrites 250. The nightly budget is a p99
//! of 50 ms per diff and 64 MiB of peak memory growth; `thresholds.json` caps
//! the same p99 for the criterion group. The bench never gates a pull request:
//! the L3 journey `ontology_diff_tests.rs::import_reports_impact_and_opens_change_request`
//! does.
//!
//! Run: `cargo bench -p medulla-h-admin --features shacl --bench ontology_diff`

use criterion::{criterion_group, criterion_main, Criterion};
use medulla_h_admin::graph_schema::ontology_diff::{diff_axioms, SchemaSide};
use std::hint::black_box;
use std::time::{Duration, Instant};

const AXIOMS: usize = 5_000;
const CHURN: usize = 250;
const RUNS: usize = 100;
const P99_BUDGET: Duration = Duration::from_millis(50);
const PEAK_MEMORY_BUDGET_KIB: u64 = 64 * 1024;

const PREFIXES: &str =
    "@prefix owl: <http://www.w3.org/2002/07/owl#> .\n@prefix ex: <http://example.org/onto#> .\n";

fn revision(label: &str, classes: impl Iterator<Item = (usize, bool)>) -> SchemaSide {
    let mut ttl = String::from(PREFIXES);
    for (index, rewritten) in classes {
        ttl.push_str(&format!("ex:C{index:05} a owl:Class .\n"));
        if rewritten {
            ttl.push_str(&format!("ex:C{index:05} owl:deprecated true .\n"));
        }
    }
    SchemaSide {
        revision: label.to_string(),
        ontology_ttl: ttl,
        shapes_ttl: None,
    }
}

fn revisions() -> (SchemaSide, SchemaSide) {
    let from = revision("1", (0..AXIOMS).map(|index| (index, false)));
    let to = revision(
        "2",
        (CHURN..AXIOMS + CHURN).map(|index| (index, index < 2 * CHURN)),
    );
    (from, to)
}

/// Peak resident set of this process in KiB, from `/proc/self/status`.
fn peak_rss_kib() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| {
            status
                .lines()
                .find_map(|line| line.strip_prefix("VmHWM:"))
                .and_then(|rest| rest.split_whitespace().next()?.parse().ok())
        })
        .unwrap_or(0)
}

fn bench_ontology_diff(c: &mut Criterion) {
    let (from, to) = revisions();
    let diff = diff_axioms(&from, &to).expect("the revisions diff");
    assert_eq!(diff.removed.len(), CHURN);
    assert_eq!(diff.added.len(), CHURN);
    assert_eq!(diff.deprecated.len(), CHURN);

    c.bench_function("ontology_diff", |b| {
        b.iter(|| black_box(diff_axioms(&from, &to).unwrap()))
    });

    let before = peak_rss_kib();
    let mut samples: Vec<Duration> = (0..RUNS)
        .map(|_| {
            let started = Instant::now();
            black_box(diff_axioms(&from, &to).unwrap());
            started.elapsed()
        })
        .collect();
    samples.sort();
    let p99 = samples[RUNS * 99 / 100];
    let growth = peak_rss_kib().saturating_sub(before);
    println!("ontology_diff p99 {p99:?} (budget {P99_BUDGET:?}), peak memory growth {growth} KiB");
    assert!(p99 <= P99_BUDGET, "p99 {p99:?} exceeds {P99_BUDGET:?}");
    assert!(
        growth <= PEAK_MEMORY_BUDGET_KIB,
        "peak memory grew {growth} KiB, over {PEAK_MEMORY_BUDGET_KIB} KiB"
    );
}

criterion_group!(benches, bench_ontology_diff);
criterion_main!(benches);
