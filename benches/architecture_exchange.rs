//! ArchiMate exchange export at 5,000 elements (MEDULLA-CQ-ORCH-R705.10).
//!
//! Nightly bench group `architecture_exchange`: `write_exchange` over one graph
//! core holding 5,000 application components, repeated for 100 samples. Budget
//! per export: p99 below 200 ms; peak resident memory (`VmHWM`) below 128 MiB.
//! The run exits non-zero when a budget is exceeded. A service level, not a pull
//! request gate.
//!
// spec: MEDULLA-CQ-ORCH-R705.10
//!
//! Run: cargo bench --bench architecture_exchange

use std::time::{Duration, Instant};

use medulla_core::architecture_exchange::write::write_exchange;
use medulla_core::architecture_exchange::{ArchimateTypeMap, ExchangeView, ExportScope};
use medulla_graph_core::graph::GraphCore;

const ELEMENTS: usize = 5_000;
const SAMPLES: usize = 100;
const BUDGET_P99: Duration = Duration::from_millis(200);
const BUDGET_MIB: f64 = 128.0;

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
    let types = ArchimateTypeMap {
        classes: [(
            "ApplicationComponent".to_string(),
            "ApplicationComponent".to_string(),
        )]
        .into(),
        edges: Default::default(),
        parents: Default::default(),
    };
    let core = GraphCore::new();
    for index in 0..ELEMENTS {
        let properties = serde_json::json!({
            "type": "ApplicationComponent",
            "name": format!("component-{index:05}"),
            "archimateIdentifier": format!("id-{index:05}"),
        });
        core.add_node(
            format!("archimate:bench:id-{index:05}"),
            rmp_serde::to_vec_named(&properties).expect("bench node"),
        );
    }
    let scope = ExportScope {
        tenant: "bench".into(),
        graph: None,
    };
    let mut samples = Vec::with_capacity(SAMPLES);
    for _ in 0..SAMPLES {
        let view = ExchangeView {
            core: &core,
            types: &types,
        };
        let started = Instant::now();
        let bytes = write_exchange(&view, &scope).expect("bench export");
        samples.push(started.elapsed());
        assert!(!bytes.is_empty());
    }
    samples.sort();
    let p99 = samples[(samples.len() * 99 / 100).min(samples.len() - 1)];
    let peak = high_water_mib();
    println!(
        "architecture_exchange p99 = {p99:?} (budget {BUDGET_P99:?}) peak_mib = {peak:.1} (budget {BUDGET_MIB})"
    );
    assert!(p99 < BUDGET_P99, "p99 {p99:?} exceeds {BUDGET_P99:?}");
    assert!(
        peak < BUDGET_MIB,
        "peak {peak:.1} MiB exceeds {BUDGET_MIB} MiB"
    );
}
