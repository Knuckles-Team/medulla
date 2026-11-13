//! Shortcut-detector bench (MEDULLA-FEDERATED-QUERY-R702.6).
//!
//! One `synthesize` call runs the four shortcut detectors (single clue, co-coverage, exposed
//! constant, prior knowledge) over an evidence graph of 10,000 nodes. The served
//! `SynthesizeSearchTask` handler calls exactly this function off-lock after it has narrowed the
//! caller's snapshot. The gate key `shortcut_detectors` in `thresholds.json` caps p50 at 6 ms and
//! p99 at 40 ms.
//!
//! Run: `cargo bench -p medulla-plan --bench shortcut_detectors`.
// spec: MEDULLA-FEDERATED-QUERY-R702.6

use criterion::{criterion_group, criterion_main, Criterion};
use medulla_plan::search_task::synthesize;
use medulla_work_types::search_task::{EvidenceEdge, EvidenceGraph, EvidenceNode, RiskThresholds};
use std::hint::black_box;

const NODES: usize = 10_000;
/// Every node also links to the node this many places ahead, so the graph is not a bare chain.
const SKIP: usize = 7;

fn id(n: usize) -> String {
    format!("n{n:06}")
}

fn evidence() -> EvidenceGraph {
    let nodes = (0..NODES)
        .map(|n| EvidenceNode {
            id: id(n),
            label: format!("label {n}"),
            text: format!("text of {n}"),
        })
        .collect();
    let mut edges = Vec::new();
    for n in 0..NODES - 1 {
        edges.push(EvidenceEdge {
            from: id(n),
            to: id(n + 1),
            relation: "linked".into(),
        });
        if n + SKIP < NODES {
            edges.push(EvidenceEdge {
                from: id(n),
                to: id(n + SKIP),
                relation: "skips".into(),
            });
        }
    }
    EvidenceGraph { nodes, edges }
}

fn bench_detectors(c: &mut Criterion) {
    let graph = evidence();
    // Lenient thresholds: no refinement round runs, so the bench measures the detectors alone.
    let thresholds = RiskThresholds {
        single_clue: 1.0,
        co_coverage: 1.0,
        exposed_constant: 1.0,
        prior_knowledge: f64::MAX,
    };
    c.bench_function("shortcut_detectors", |b| {
        b.iter(|| {
            let out = synthesize(black_box(&graph), "n000000", &thresholds, 0)
                .expect("the answer node is in the graph");
            black_box(out.report.findings.len());
        });
    });
}

criterion_group!(benches, bench_detectors);
criterion_main!(benches);
