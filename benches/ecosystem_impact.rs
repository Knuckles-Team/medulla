//! MEDULLA-SUPPLY-VOCAB-R009.6 nightly group `decide/ecosystem_impact`
//! (MEDULLA-DECISION-ENGINE-R178.1): one finding propagated over a
//! 10,000-package dependency graph.
//!
//! Budget (spec): p99 under 50 ms per propagation. Package `pkg-i` depends on
//! `pkg-(i-1)/2` and, for every tenth package, also on `pkg-i-1` (a diamond
//! per ten packages), so every package is reachable from `pkg-0` within the
//! 64-hop maximum of the `ecosystem.max_dependency_depth` knob.
//! Deterministic: fixed ids, no RNG. Benchmarks never gate a PR.
//!
//! Run: cargo bench --bench ecosystem_impact

use std::time::Instant;

use medulla_core::ecosystem::{
    inherit_severity, propagate_package_risk_with, PackageFinding, Severity, DEPENDS_ON,
};
use medulla_graph_core::graph::GraphCore;

const GROUP: &str = "decide/ecosystem_impact";
const PACKAGES: usize = 10_000;
const MAX_DEPTH: usize = 64;
const ITERATIONS: usize = 50;
const WARMUP: usize = 3;
const P99_BUDGET_MS: f64 = 50.0;

fn package(index: usize) -> String {
    format!("pkg-{index}")
}

fn dependency_graph() -> GraphCore {
    let graph = GraphCore::new();
    let empty = rmp_serde::to_vec_named(&serde_json::json!({"type": "EcosystemPackage"}))
        .expect("node props encode");
    let depends = rmp_serde::to_vec_named(&serde_json::json!({"relationship": DEPENDS_ON}))
        .expect("edge props encode");
    for index in 0..PACKAGES {
        graph.add_node(package(index), empty.clone());
    }
    for index in 1..PACKAGES {
        graph
            .add_edge(package(index), package((index - 1) / 2), depends.clone())
            .expect("dependency edge");
        if index % 10 == 0 {
            graph
                .add_edge(package(index), package(index - 1), depends.clone())
                .expect("diamond edge");
        }
    }
    graph
}

fn percentile(sorted: &[f64], fraction: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    sorted[((sorted.len() - 1) as f64 * fraction).round() as usize]
}

fn main() {
    let graph = dependency_graph();
    let finding = PackageFinding {
        id: "finding-bench".into(),
        severity: Severity::Critical,
    };
    let run = || {
        propagate_package_risk_with(&graph, "pkg-0", &finding, MAX_DEPTH, inherit_severity)
            .expect("pkg-0 is present")
    };
    let reached = run().len();
    assert_eq!(reached, PACKAGES - 1, "every package depends on pkg-0");
    for _ in 0..WARMUP {
        let _ = run();
    }
    let mut samples = Vec::with_capacity(ITERATIONS);
    for _ in 0..ITERATIONS {
        let start = Instant::now();
        let impacts = run();
        samples.push(start.elapsed().as_secs_f64() * 1_000.0);
        assert_eq!(impacts.len(), reached);
    }
    samples.sort_by(|a, b| a.total_cmp(b));
    let p50 = percentile(&samples, 0.50);
    let p99 = percentile(&samples, 0.99);
    println!(
        "{GROUP}: packages={PACKAGES} reached={reached} p50={p50:.3}ms p99={p99:.3}ms budget_p99={P99_BUDGET_MS}ms within_budget={}",
        p99 < P99_BUDGET_MS
    );
}
