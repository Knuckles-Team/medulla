//! MEDULLA-SDLC-R014.12 — hot-path budget of the spec history ingest.
//!
//! `project_spec_artifacts` → `link_history_to_rows`: a delta of 100 commits over a
//! 20000-row repository must stay under 2 s, and a first ingest of 25 repositories of
//! 800 rows each (20000 rows) under 120 s. The benchmark gates only the nightly run.
//!
//! Run: cargo bench -p medulla-h-ingest --bench lifecycle_spec_history_ingest

use std::collections::BTreeSet;
use std::hint::black_box;
use std::time::{Duration, Instant};

use criterion::{criterion_group, criterion_main, Criterion};
use medulla_core::lifecycle::derive::TestBinding;
use medulla_h_ingest::source_ingestion::spec_projection::{
    link_history_to_rows, SpecHistoryInput, SpecRowLinks,
};
use medulla_source_types::source_changeset::{Changeset, Trailers};

const DELTA_BUDGET: Duration = Duration::from_secs(2);
const FIRST_INGEST_BUDGET: Duration = Duration::from_secs(120);

fn ids(rows: usize) -> Vec<String> {
    (0..rows).map(|n| format!("BENCH-R{n:05}")).collect()
}

fn changesets(count: usize, rows: usize) -> Vec<Changeset> {
    (0..count)
        .map(|n| Changeset {
            id: format!("c{n}"),
            repo: "bench".into(),
            parents: Vec::new(),
            author_principal: "dev".into(),
            message: "change".into(),
            changed_paths: Vec::new(),
            created_at: n as u64,
            trailers: Trailers {
                spec_ids: vec![format!("BENCH-R{:05}", n % rows)],
                ..Default::default()
            },
            agent_identity: None,
            run_trace_id: None,
            git: None,
            symbol_edit: None,
        })
        .collect()
}

fn links(rows: &[String]) -> Vec<SpecRowLinks> {
    rows.iter()
        .enumerate()
        .skip(1)
        .map(|(n, id)| SpecRowLinks {
            row_id: id.clone(),
            depends_on: vec![rows[n - 1].clone()],
            ..SpecRowLinks::default()
        })
        .collect()
}

fn bindings(rows: &[String]) -> Vec<TestBinding> {
    rows.iter()
        .step_by(2)
        .map(|id| TestBinding {
            requirement: id.clone(),
            test: format!("tests/{id}.py"),
        })
        .collect()
}

fn run(repo: &str, rows: &[String], chain: &[Changeset], delta: &[Changeset]) {
    let links = links(rows);
    let bindings = bindings(rows);
    let existing: BTreeSet<String> = bindings.iter().map(|b| b.test.clone()).collect();
    let projection = link_history_to_rows(&SpecHistoryInput {
        repo,
        rev: "head",
        ids: rows,
        links: &links,
        chain,
        new_changesets: delta,
        bindings: &bindings,
        existing_test_paths: &existing,
    })
    .expect("every trailer names a row");
    black_box(projection);
}

fn spec_history_ingest(c: &mut Criterion) {
    let rows = ids(20_000);
    let chain = changesets(1_000, rows.len());
    let started = Instant::now();
    run("bench", &rows, &chain, &chain[900..]);
    let delta = started.elapsed();
    assert!(delta <= DELTA_BUDGET, "delta {delta:?} over the 2 s budget");

    let started = Instant::now();
    for repo in 0..25 {
        let rows = ids(800);
        let chain = changesets(200, rows.len());
        run(&format!("bench{repo}"), &rows, &chain, &chain);
    }
    let first = started.elapsed();
    assert!(
        first <= FIRST_INGEST_BUDGET,
        "first ingest {first:?} over the 120 s budget"
    );

    let mut group = c.benchmark_group("lifecycle/spec_history_ingest");
    group.sample_size(10);
    group.bench_function("delta_100_commits", |b| {
        b.iter(|| run("bench", &rows, &chain, &chain[900..]))
    });
    group.finish();
}

criterion_group!(benches, spec_history_ingest);
criterion_main!(benches);
