//! One bounded neighborhood read at 1,000,000 edges (MEDULLA-DURABLE-KERNEL-R876.6).
//!
//! Nightly bench group `graph/neighborhood`: `walk` reads depth 2 from a hub with
//! `total_limit` 500 over a graph of 1,000,000 edges. Budget per read: p95 at most 50 ms.
//! The run exits non-zero when the budget is exceeded. A service level, not a pull
//! request gate.
//!
// spec: MEDULLA-DURABLE-KERNEL-R876.6
//!
//! Run: cargo bench --bench neighborhood

use std::time::{Duration, Instant};

use medulla_graph_core::graph::GraphCore;
use medulla_graph_core::neighborhood::{walk, NeighborhoodQuery};

const NODES: usize = 250_000;
const EDGES: usize = 1_000_000;
const READS: usize = 100;
const BUDGET_P95: Duration = Duration::from_millis(50);

fn props(relationship: &str) -> Vec<u8> {
    rmp_serde::to_vec_named(&serde_json::json!({ "relationship": relationship }))
        .expect("encode edge properties")
}

/// A deterministic graph: node `n` links to `(n * 7 + step) % NODES` for four steps, and
/// node 0 is a hub that links to every 100th node.
fn graph() -> GraphCore {
    let core = GraphCore::new();
    let empty = rmp_serde::to_vec_named(&serde_json::json!({})).expect("encode node");
    for n in 0..NODES {
        core.add_node_no_ledger(format!("n{n}"), empty.clone());
    }
    let link = props("LINKS");
    let mut edges = 0;
    for n in 0..NODES {
        for step in 1..=4usize {
            let target = (n * 7 + step) % NODES;
            if target != n && edges < EDGES {
                core.add_edge_no_ledger(format!("n{n}"), format!("n{target}"), link.clone())
                    .expect("add edge");
                edges += 1;
            }
        }
    }
    for n in (100..NODES).step_by(100) {
        if edges >= EDGES {
            break;
        }
        core.add_edge_no_ledger("n0".to_string(), format!("n{n}"), link.clone())
            .expect("add hub edge");
        edges += 1;
    }
    core
}

fn main() {
    let core = graph();
    let types = vec!["LINKS".to_string()];
    let mut samples = Vec::with_capacity(READS);
    for _ in 0..READS {
        let query = NeighborhoodQuery {
            root: "n0",
            depth: 2,
            edge_types: &types,
            per_node_limit: 50,
            total_limit: 500,
            exclude: &[],
            skip: 0,
        };
        let started = Instant::now();
        let walked = walk(&core, &query).expect("walk");
        samples.push(started.elapsed());
        assert!(walked.nodes.len() > 1 && walked.nodes.len() <= 501);
    }
    samples.sort();
    let p95 = samples[(samples.len() * 95 / 100).min(samples.len() - 1)];
    println!("graph/neighborhood p95 = {p95:?} (budget {BUDGET_P95:?})");
    assert!(p95 <= BUDGET_P95, "p95 {p95:?} exceeds {BUDGET_P95:?}");
}
