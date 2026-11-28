//! Marking propagation over 100,000 lineage nodes (MEDULLA-TYPED-PACKS-R652.6).
//!
//! Nightly bench group `classification/marking_propagate`: `propagate` walks a
//! 100,000-asset lineage tree (fan-out 4) from its root at the default depth
//! bound. Budget per walk: p99 below 8 ms. The run exits non-zero when the
//! budget is exceeded. A service level, not a pull request gate.
//!
// spec: MEDULLA-TYPED-PACKS-R652.6
//!
//! Run: cargo bench --bench marking_propagate

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use medulla_identity_core_types::tenancy::Marking;
use medulla_isolation::isolation::marking_propagation::{propagate, LineageEdges};

const NODES: usize = 100_000;
const FAN_OUT: usize = 4;
const WALKS: usize = 200;
const BUDGET_P99: Duration = Duration::from_millis(8);

fn tree() -> LineageEdges {
    let mut lineage = LineageEdges::new();
    for child in 1..NODES {
        let parent = (child - 1) / FAN_OUT;
        lineage
            .entry(format!("asset-{parent}"))
            .or_default()
            .insert(format!("asset-{child}"));
    }
    lineage
}

fn main() {
    let lineage = tree();
    let categories = [Marking::new("pii").expect("a valid marking")];
    let none = BTreeSet::new();
    let mut samples = Vec::with_capacity(WALKS);
    for _ in 0..WALKS {
        let started = Instant::now();
        let reached = propagate(&lineage, "asset-0", &categories, &[], 64, &none);
        samples.push(started.elapsed());
        assert_eq!(reached.len(), NODES - 1);
    }
    samples.sort();
    let p99 = samples[(samples.len() * 99 / 100).min(samples.len() - 1)];
    println!("classification/marking_propagate p99 = {p99:?} (budget {BUDGET_P99:?})");
    assert!(p99 < BUDGET_P99, "p99 {p99:?} exceeds {BUDGET_P99:?}");
}
