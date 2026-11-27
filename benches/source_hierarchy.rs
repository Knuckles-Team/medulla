//! Container subtree resolution at 10,000 containers (MEDULLA-INGEST-WORKER-R904.6).
//!
//! Nightly bench group `ingest/source_hierarchy`: `source_hierarchy::subtree` resolves the whole
//! subtree of a root over 10,000 containers, the call the scope resolution of
//! MEDULLA-FEDERATED-QUERY-R704 makes. Budget per resolution: p99 at most 5 ms; peak resident
//! memory (`VmHWM`) at most 32 MiB. The run exits non-zero when a budget is exceeded. A service
//! level, not a pull request gate.
//!
// spec: MEDULLA-INGEST-WORKER-R904.6
//!
//! Run: cargo bench --bench source_hierarchy

use std::time::{Duration, Instant};

use medulla_core::source_hierarchy::{container_node_id, subtree, upsert_containers};
use medulla_graph_core::graph::GraphCore;
use medulla_ingestion_types::source_governance::SourceContainer;

const CONTAINERS: usize = 10_000;
const FIRST_LEVEL: usize = 100;
const RESOLUTIONS: usize = 100;
/// The fixed clock reading every container is stamped with, so the tree is identical per run.
const SYNCED_AT_MS: u64 = 1_700_000_000_000;
const BUDGET_P99: Duration = Duration::from_millis(5);
const BUDGET_MIB: f64 = 32.0;

fn container(id: String, parent: Option<String>) -> SourceContainer {
    SourceContainer {
        source: "drive".into(),
        title: format!("title {id}"),
        container_id: id,
        parent_id: parent,
    }
}

/// One root, `FIRST_LEVEL` folders under it and the rest spread over those folders.
fn tree() -> Vec<SourceContainer> {
    let mut containers = vec![container("root".into(), None)];
    for folder in 0..FIRST_LEVEL {
        containers.push(container(format!("folder-{folder}"), Some("root".into())));
    }
    for leaf in 0..(CONTAINERS - 1 - FIRST_LEVEL) {
        containers.push(container(
            format!("leaf-{leaf}"),
            Some(format!("folder-{}", leaf % FIRST_LEVEL)),
        ));
    }
    containers
}

/// Resident high-water mark in MiB (`VmHWM`), 0 where procfs is absent.
fn high_water_mib() -> f64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| {
            status
                .lines()
                .find(|line| line.starts_with("VmHWM:"))
                .and_then(|line| line.split_whitespace().nth(1)?.parse::<f64>().ok())
        })
        .map_or(0.0, |kib| kib / 1024.0)
}

fn main() {
    let core = GraphCore::new();
    let written =
        upsert_containers(&core, "tenant:bench", &tree(), SYNCED_AT_MS).expect("the tree is valid");
    assert_eq!(written.nodes, CONTAINERS as u64);
    let root = container_node_id("tenant:bench", "drive", "root").expect("root id");
    let mut samples = Vec::with_capacity(RESOLUTIONS);
    for _ in 0..RESOLUTIONS {
        let started = Instant::now();
        let resolved = subtree(&core, &root, 32).expect("the root is a container");
        samples.push(started.elapsed());
        assert_eq!(resolved.len(), CONTAINERS);
    }
    samples.sort();
    let p99 = samples[(samples.len() * 99 / 100).min(samples.len() - 1)];
    let peak = high_water_mib();
    println!(
        "ingest/source_hierarchy p99 = {p99:?} (budget {BUDGET_P99:?}) peak_mib = {peak:.1} (budget {BUDGET_MIB})"
    );
    assert!(p99 <= BUDGET_P99, "p99 {p99:?} exceeds {BUDGET_P99:?}");
    assert!(
        peak <= BUDGET_MIB,
        "peak {peak:.1} MiB exceeds {BUDGET_MIB} MiB"
    );
}
