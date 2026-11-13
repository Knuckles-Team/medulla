//! Tag-based column masking hot-path proof (MEDULLA-FEDERATED-QUERY-R706.6).
//!
//! Projects 4 columns over 1,000,000 nodes through `execute` twice: once raw, once with
//! a bound `PolicySet` that masks all four (the rewrite `execute` applies at its top,
//! the call on the served `Uql` chain). The masked run may cost at most 8 percent more
//! than the unmasked scan; a regression above 10 percent fails the nightly run, and the
//! bench never gates a pull request. The criterion group `policy_mask_4col_1m` is also
//! capped in `thresholds.json`.
//!
//! Run: `cargo bench -p medulla-plan --features query --bench policy_mask`
// spec: MEDULLA-FEDERATED-QUERY-R706.6

use criterion::{criterion_group, criterion_main, Criterion};
use medulla_exec::policy_mask::{MaskMethod, MaskPolicy, PolicySet};
use medulla_exec::{algebra::Op, algebra::Plan, exec::execute, exec::PlanCtx};
use medulla_graph_core::graph::GraphCore;
use medulla_semantic_index::semantic::SemanticStore;
use serde_json::json;
use std::hint::black_box;
use std::time::{Duration, Instant};

const ROWS: usize = 1_000_000;
const COLUMNS: [&str; 4] = ["pii.email", "pii.ssn", "pii.dob", "pii.home"];
const BUDGET: f64 = 0.08;
const FAIL_ABOVE: f64 = 0.10;
const RUNS: usize = 7;

fn policies() -> PolicySet {
    let methods = [
        MaskMethod::Null,
        MaskMethod::Hash,
        MaskMethod::Partial,
        MaskMethod::Redact,
    ];
    PolicySet {
        masks: COLUMNS
            .iter()
            .zip(methods)
            .map(|(tag, method)| MaskPolicy {
                tag: (*tag).into(),
                principals_exempt: Vec::new(),
                method,
            })
            .collect(),
        row_filters: Vec::new(),
    }
}

fn plan() -> Plan {
    Plan::new(vec![
        Op::Scan {
            label: "Person".into(),
        },
        Op::Project {
            channels: COLUMNS.iter().map(|c| c.to_string()).collect(),
        },
    ])
}

/// Median wall time of `RUNS` executions of `plan`.
fn median(plan: &Plan, ctx: &PlanCtx) -> Duration {
    let mut samples: Vec<Duration> = (0..RUNS)
        .map(|_| {
            let started = Instant::now();
            black_box(execute(plan, ctx).expect("plan executes"));
            started.elapsed()
        })
        .collect();
    samples.sort();
    samples[RUNS / 2]
}

fn bench_policy_mask(c: &mut Criterion) {
    let core = GraphCore::new();
    for row in 0..ROWS {
        let id = format!("p{row:07}");
        let props = json!({"type": "Person", "key": id, "pii.email": "a@b.c"});
        core.add_node(id, rmp_serde::to_vec_named(&props).unwrap());
    }
    let view = core.analysis_snapshot();
    let semantic = SemanticStore::new();
    let set = policies();
    let raw_ctx = PlanCtx::new(&view, &semantic);
    let masked_ctx = PlanCtx::new(&view, &semantic)
        .with_policies(&set)
        .with_principal("analyst");
    let plan = plan();

    let mut group = c.benchmark_group("policy_mask_4col_1m");
    group.sample_size(10);
    group.bench_function("unmasked", |b| {
        b.iter(|| black_box(execute(&plan, &raw_ctx).unwrap()))
    });
    group.bench_function("masked", |b| {
        b.iter(|| black_box(execute(&plan, &masked_ctx).unwrap()))
    });
    group.finish();

    let raw = median(&plan, &raw_ctx).as_secs_f64();
    let masked = median(&plan, &masked_ctx).as_secs_f64();
    let overhead = masked / raw - 1.0;
    println!(
        "policy_mask overhead {:.2}% (budget {:.0}%, nightly fails above {:.0}%)",
        overhead * 100.0,
        BUDGET * 100.0,
        FAIL_ABOVE * 100.0
    );
    assert!(
        overhead <= FAIL_ABOVE,
        "masking overhead {:.2}% exceeds the {:.0}% regression limit",
        overhead * 100.0,
        FAIL_ABOVE * 100.0
    );
}

criterion_group!(benches, bench_policy_mask);
criterion_main!(benches);
