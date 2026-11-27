//! MEDULLA-SDLC-R016.1, R016.5 and R017.7 -- hot-path budgets of the spec research and
//! proposal journeys.
//!
//! `lifecycle/research_trigger`: a plan whose Phase 0 section holds 1,000 items is projected
//! into `PlanUnknown` records, each unknown becomes one frontier option and one work item key.
//! The p99 of the whole pass must stay at most 100 ms.
//!
//! `lifecycle/proposals`: one proposal is written, one loop-authored spec edit is judged against
//! the stored proposals and one commit message is read for its `Proposal:` trailers. The p99 of
//! each step must stay at most 300 ms. The benchmarks gate only the nightly run, never a pull
//! request.
//!
//! `lifecycle/spec_ingest` (MEDULLA-SDLC-R014.9, the budgets of R014.2): one changed file of a
//! repository with 2,000 rows and 2,000 task lines is re-ingested by digest; only that file is
//! parsed. The p99 must stay at most one second.
//!
//! Run: cargo bench -p medulla-h-ingest --bench spec_ingest

use std::collections::{BTreeMap, BTreeSet};
use std::hint::black_box;
use std::time::{Duration, Instant};

use criterion::{criterion_group, criterion_main, Criterion};
use medulla_h_ingest::source_ingestion::spec_projection::{
    plan_unknowns, project_plans, reingest_changed_artifacts, StoredArtifact,
};
use medulla_sdlc::lifecycle::proposals::{check_changeset_provenance, StoredBinding};
use medulla_sdlc::lifecycle::research::{research_item_key, unknown_frontier};
use medulla_source_types::lifecycle::{
    handle_proposal_put, proposal_trailers, EvidenceKind, Proposal, ProposalEvidence, ProposalKind,
    ProposalOrigin, ProposalState,
};

const ITEMS: usize = 1_000;
const SAMPLES: usize = 200;
const RESEARCH_P99: Duration = Duration::from_millis(100);
const PROPOSAL_P99: Duration = Duration::from_millis(300);
const INGEST_P99: Duration = Duration::from_secs(1);
const ROWS: usize = 2_000;

fn plan() -> String {
    let mut text = String::from("# Plan\n\n## Phase 0: Research\n");
    for n in 0..ITEMS {
        text.push_str(&format!("- Unknown number {n} of the plan\n"));
    }
    text.push_str("\n## Phase 1: Design\n- Not an unknown\n");
    text
}

fn p99(samples: &mut [Duration]) -> Duration {
    samples.sort();
    samples[((samples.len() - 1) as f64 * 0.99).ceil() as usize]
}

fn research_trigger(c: &mut Criterion) {
    let text = plan();
    let end = text.lines().count() as u32;
    let known = BTreeSet::new();
    let mut samples: Vec<Duration> = (0..SAMPLES)
        .map(|_| {
            let started = Instant::now();
            let projected = project_plans("demo", "specs/x/plan.md", black_box(&text), &known)
                .expect("the plan projects");
            let unknowns = plan_unknowns(black_box(&text), 3, end);
            for unknown in &unknowns {
                let id = format!("planunknown:demo:specs/x/plan.md#L{}", unknown.line);
                black_box(unknown_frontier(&id, &unknown.text));
                black_box(research_item_key(&id, &unknown.text));
            }
            black_box((projected, unknowns));
            started.elapsed()
        })
        .collect();
    let measured = p99(&mut samples);
    assert!(
        measured <= RESEARCH_P99,
        "p99 {measured:?} over the 100 ms budget"
    );
    let mut group = c.benchmark_group("lifecycle/research_trigger");
    group.bench_function("plan_unknowns", |b| {
        b.iter(|| plan_unknowns(black_box(&text), 3, end))
    });
    group.finish();
}

fn evidence() -> Vec<ProposalEvidence> {
    vec![ProposalEvidence {
        kind: EvidenceKind::CodeGraphFact,
        record_id: "fact:a".into(),
        digest: "d".repeat(64),
    }]
}

fn proposal(n: usize) -> Proposal {
    Proposal {
        project: "demo".into(),
        kind: ProposalKind::RowUpdate,
        target: format!("DEMO-R{n:04}"),
        statement: "Tighten the row.".into(),
        marker: None,
        evidence: evidence(),
        origin: ProposalOrigin::Requested,
    }
}

fn proposals(c: &mut Criterion) {
    let resolved = vec![("fact:a".to_string(), "d".repeat(64))];
    let ids: Vec<String> = (0..ITEMS).map(|n| format!("proposal:{n}")).collect();
    let stored: Vec<StoredBinding<'_>> = ids
        .iter()
        .map(|id| StoredBinding {
            id,
            project: "demo",
            state: ProposalState::Approved,
        })
        .collect();
    let named = vec![ids[ITEMS - 1].clone()];
    let message = format!(
        "feat: edit the row\n\nSpec: MEDULLA-SDLC-R017.7\nProposal: {}",
        ids[ITEMS - 1]
    );
    let touched = ["specs/demo/requirements.md"];
    let mut puts: Vec<Duration> = Vec::new();
    let mut guards: Vec<Duration> = Vec::new();
    let mut reads: Vec<Duration> = Vec::new();
    for n in 0..SAMPLES {
        let input = proposal(n);
        let started = Instant::now();
        black_box(handle_proposal_put(black_box(input), &resolved)).expect("the proposal writes");
        puts.push(started.elapsed());
        let started = Instant::now();
        black_box(check_changeset_provenance(
            true, "demo", &touched, &named, &stored,
        ))
        .expect("an approved proposal lands");
        guards.push(started.elapsed());
        let started = Instant::now();
        black_box(proposal_trailers(black_box(&message)));
        reads.push(started.elapsed());
    }
    for (name, samples) in [
        ("put", &mut puts),
        ("guard", &mut guards),
        ("trailers", &mut reads),
    ] {
        let measured = p99(samples);
        assert!(
            measured <= PROPOSAL_P99,
            "{name}: p99 {measured:?} over the 300 ms budget"
        );
    }
    let mut group = c.benchmark_group("lifecycle/proposals");
    group.bench_function("guard", |b| {
        b.iter(|| check_changeset_provenance(true, "demo", &touched, &named, &stored))
    });
    group.bench_function("put", |b| {
        let mut n = 0;
        b.iter(|| {
            n += 1;
            handle_proposal_put(black_box(proposal(n)), &resolved)
        })
    });
    group.finish();
}

fn requirements_text() -> String {
    let mut text =
        String::from("# Requirements\n\n| ID | Requirement | Acceptance |\n|----|----|----|\n");
    for n in 1..=ROWS {
        text.push_str(&format!(
            "| `DEMO-R{n:04}` | **Row {n}.** Caller chain: `serve_{n}` -> `src/demo.py:{n}`. | \
Test: `tests/test_demo.py::test_row_{n}` (L3). |\n"
        ));
    }
    text
}

fn tasks_text(ticked: usize) -> String {
    let mut text = String::from("# Tasks\n\n## Phase 1: Build\n");
    for n in 1..=ROWS {
        let box_ = if n <= ticked { 'x' } else { ' ' };
        text.push_str(&format!("- [{box_}] T{n:04} DEMO-R{n:04} Build row {n}\n"));
    }
    text
}

fn stored_of(
    plan: &medulla_h_ingest::source_ingestion::spec_projection::ReingestPlan,
) -> BTreeMap<String, StoredArtifact> {
    plan.operations
        .iter()
        .filter(|op| op["op"] == "upsert_node" && op["properties"]["type"] == "SpecArtifactFile")
        .map(|op| {
            let properties = &op["properties"];
            let records = properties["records"].as_str().unwrap_or_default();
            (
                properties["file"].as_str().unwrap_or_default().to_string(),
                StoredArtifact {
                    digest: properties["digest"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                    records: records
                        .split(',')
                        .filter(|id| !id.is_empty())
                        .map(str::to_string)
                        .collect(),
                    terms: Vec::new(),
                },
            )
        })
        .collect()
}

fn spec_ingest(c: &mut Criterion) {
    let requirements = requirements_text();
    let (open, ticked) = (tasks_text(0), tasks_text(1));
    let none = BTreeMap::new();
    let first = reingest_changed_artifacts(
        "demo",
        &[
            ("specs/demo/requirements.md", requirements.as_str()),
            ("specs/demo/tasks.md", open.as_str()),
        ],
        &BTreeMap::new(),
        &none,
        &none,
    )
    .expect("the repository ingests");
    let stored = stored_of(&first);
    let changed = [
        ("specs/demo/requirements.md", requirements.as_str()),
        ("specs/demo/tasks.md", ticked.as_str()),
    ];
    let mut samples: Vec<Duration> = (0..20)
        .map(|_| {
            let started = Instant::now();
            let plan = reingest_changed_artifacts("demo", &changed, &stored, &none, &none)
                .expect("the one-file change ingests");
            assert_eq!(plan.reingested, ["specs/demo/tasks.md"]);
            black_box(plan);
            started.elapsed()
        })
        .collect();
    let measured = p99(&mut samples);
    assert!(
        measured <= INGEST_P99,
        "p99 {measured:?} over the one second budget"
    );
    let mut group = c.benchmark_group("lifecycle/spec_ingest");
    group.sample_size(10);
    group.bench_function("one_file_change", |b| {
        b.iter(|| reingest_changed_artifacts("demo", &changed, &stored, &none, &none))
    });
    group.finish();
}

criterion_group!(benches, research_trigger, proposals, spec_ingest);
criterion_main!(benches);
