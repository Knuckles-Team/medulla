//! Topology analytics bench (MEDULLA-FEDERATED-QUERY-R607.6).
//!
//! Three gate keys of `thresholds.json`, enforced by `scripts/bench_gate.py`:
//! `spectral_100k` (one spectral partition of a 100,000-node graph), `analogy_10k_edges`
//! (one analogy search over a labelled graph of 10,000 edges) and `mincut_10k` (one global
//! minimum cut of a graph of 10,000 edges). Every fixture is deterministic, and each run's
//! answer is checked once before it is timed.
//!
//! Run: `cargo bench -p medulla-graph-algos --bench topology`.
// spec: MEDULLA-FEDERATED-QUERY-R607.6

use criterion::{criterion_group, criterion_main, Criterion};
use medulla_graph_algos::graph_algos::spectral::{checked_mincut, spectral_partition};
use medulla_graph_algos::graph_algos::{analogy_matches, AdjacencyGraph, LabeledGraph};
use std::hint::black_box;

/// Clusters of the spectral fixture and nodes in each: 100 x 1,000 = 100,000 nodes.
const CLUSTERS: usize = 100;
const CLUSTER_NODES: usize = 1_000;
/// Partitions requested of the spectral run.
const PARTS: usize = 8;

/// Each node links to the next three nodes of its cluster ring; one edge per cluster
/// joins it to the next cluster, so the graph is connected and clustered.
fn clustered() -> AdjacencyGraph<u32> {
    let node =
        |cluster: usize, index: usize| (cluster * CLUSTER_NODES + index % CLUSTER_NODES) as u32;
    let mut edges = Vec::new();
    for cluster in 0..CLUSTERS {
        for index in 0..CLUSTER_NODES {
            for step in 1..=3 {
                edges.push((node(cluster, index), node(cluster, index + step), 1.0));
            }
        }
        edges.push((node(cluster, 0), node((cluster + 1) % CLUSTERS, 0), 1.0));
    }
    AdjacencyGraph::from_edges(edges)
}

/// 2,500 nodes of two labels and exactly 10,000 edges (four relationships per node).
fn labelled() -> LabeledGraph {
    const NODES: usize = 2_500;
    let nodes = (0..NODES)
        .map(|n| {
            (
                format!("n{n:05}"),
                if n % 2 == 0 { "left" } else { "right" }.to_string(),
            )
        })
        .collect();
    let edges = (0..NODES)
        .flat_map(|n| {
            (1..=4).map(move |step| {
                (
                    format!("n{n:05}"),
                    format!("n{:05}", (n + step * 7) % NODES),
                    format!("rel{}", step % 3),
                )
            })
        })
        .collect();
    LabeledGraph { nodes, edges }
}

/// 1,000 nodes and 10,000 edges: ten ring neighbours each, so the minimum cut is a node's
/// degree and the cut is the same on every run.
fn dense_ring() -> AdjacencyGraph<u32> {
    const NODES: u32 = 1_000;
    let edges: Vec<(u32, u32, f64)> = (0..NODES)
        .flat_map(|n| (1..=10).map(move |step| (n, (n + step) % NODES, 1.0)))
        .collect();
    AdjacencyGraph::from_edges(edges)
}

fn bench_topology(c: &mut Criterion) {
    let graph = clustered();
    let warm = spectral_partition(&graph, PARTS, 7).expect("spectral fixture is valid");
    assert!(warm.partitions.len() >= 2);
    c.bench_function("spectral_100k", |b| {
        b.iter(|| {
            black_box(
                spectral_partition(black_box(&graph), PARTS, 7)
                    .unwrap()
                    .partitions
                    .len(),
            )
        });
    });

    let labelled = labelled();
    assert!(!analogy_matches(&labelled, 0.5, 100).is_empty());
    c.bench_function("analogy_10k_edges", |b| {
        b.iter(|| black_box(analogy_matches(black_box(&labelled), 0.5, 100).len()));
    });

    let ring = dense_ring();
    assert!(checked_mincut(&ring).is_ok());
    c.bench_function("mincut_10k", |b| {
        b.iter(|| black_box(checked_mincut(black_box(&ring)).is_ok()));
    });
}

criterion_group! {
    name = benches;
    config = Criterion::default().sample_size(10);
    targets = bench_topology
}
criterion_main!(benches);
