//! MEDULLA-SDLC-R015.2 — hot-path budget of the run-context assembly.
//!
//! `read_task_context` over a graph of one requirement row, its plan sections, 5000
//! contract sections of which a few name the row, and a code neighbourhood: p99 stays
//! under 150 ms. The benchmark gates only the nightly run.
//!
//! Run: cargo bench -p medulla-h-decide --bench task_context

use std::hint::black_box;
use std::time::{Duration, Instant};

use criterion::{criterion_group, criterion_main, Criterion};
use medulla_graph_core::graph::GraphCore;
use medulla_h_decide::decide::read_task_context;

const BUDGET_P99: Duration = Duration::from_millis(150);
const MAX_BYTES: usize = 65_536;
const SAMPLES: usize = 200;

fn put(core: &GraphCore, id: &str, properties: serde_json::Value) {
    core.put_node(
        id.to_string(),
        rmp_serde::to_vec_named(&properties).expect("properties encode"),
    );
}

fn edge(core: &GraphCore, source: &str, target: &str, relationship: &str) {
    core.add_edge(
        source.to_string(),
        target.to_string(),
        rmp_serde::to_vec_named(&serde_json::json!({"relationship": relationship}))
            .expect("edge encodes"),
    )
    .expect("endpoints exist");
}

fn graph() -> GraphCore {
    let core = GraphCore::new();
    let requirement = "requirement:bench:BENCH-R001";
    put(
        &core,
        requirement,
        serde_json::json!({"type": "Requirement", "text": "Serve the row."}),
    );
    for line in 0..8 {
        let plan = format!("plansection:bench:specs/a/plan.md#L{line}");
        put(
            &core,
            &plan,
            serde_json::json!({
                "type": "PlanSection", "title": "Plan", "start_line": line.to_string(),
                "text": "Plan the row. ".repeat(40),
            }),
        );
        edge(&core, &plan, requirement, "covers");
    }
    for n in 0..5_000 {
        let names_row = n % 500 == 0;
        put(
            &core,
            &format!("specsection:bench:specs/a/contracts/c{n}.md#L1"),
            serde_json::json!({
                "type": "SpecSection", "repo": "bench",
                "file": format!("specs/a/contracts/c{n}.md"),
                "title": format!("Seam {n}"), "start_line": "1",
                "text": if names_row { "Provider: BENCH-R001 seam".to_string() } else { "Other seam".to_string() },
            }),
        );
    }
    for n in 0..40 {
        let symbol = format!("symbol:serve_{n}");
        put(
            &core,
            &symbol,
            serde_json::json!({"type": "CodeSymbolRef", "name": format!("serve_{n}")}),
        );
        edge(&core, requirement, &symbol, "implementedBy");
        let caller = format!("symbol:caller_{n}");
        put(
            &core,
            &caller,
            serde_json::json!({"type": "CodeSymbolRef", "name": format!("caller_{n}")}),
        );
        edge(&core, &caller, &symbol, "calls");
    }
    core
}

fn read(core: &GraphCore) {
    let context = read_task_context(core, &|_, _| true, "bench", "BENCH-R001", MAX_BYTES)
        .expect("the row has a plan section");
    black_box(context);
}

fn task_context(c: &mut Criterion) {
    let core = graph();
    read(&core);
    let mut samples: Vec<Duration> = (0..SAMPLES)
        .map(|_| {
            let started = Instant::now();
            read(&core);
            started.elapsed()
        })
        .collect();
    samples.sort();
    let p99 = samples[SAMPLES * 99 / 100];
    assert!(p99 <= BUDGET_P99, "p99 {p99:?} over the 150 ms budget");

    let mut group = c.benchmark_group("decide/task_context");
    group.sample_size(10);
    group.bench_function("read_task_context", |b| b.iter(|| read(&core)));
    group.finish();
}

criterion_group!(benches, task_context);
criterion_main!(benches);
