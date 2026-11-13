//! Data-prep declaration at writer concurrency (MEDULLA-INGEST-WORKER-R001.1011).
//!
//! Nightly bench group `ingest/data_prep_declare`: 16 concurrent writers call
//! `declare_data_prep` for 1,000 declarations of 50 fields each against one
//! shared graph core, committing each write-set. Budget per call: p50 below 2 ms,
//! p99 below 10 ms; peak resident memory (`VmHWM`) below 128 MiB. The run exits
//! non-zero when a budget is exceeded. A service level, not a pull request gate.
//!
// spec: MEDULLA-INGEST-WORKER-R001.1011
//!
//! Run: cargo bench --bench data_prep_declare

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use medulla_graph_core::graph::GraphCore;
use medulla_h_ingest::data_prep_declaration::{declare_data_prep, CoreDeclarationView, Declared};
use medulla_protocol_types::protocol::Method;
use medulla_types::contract::Digest256;

const DECLARATIONS: usize = 1_000;
const FIELDS: usize = 50;
const WRITERS: usize = 16;
const BUDGET_P50: Duration = Duration::from_millis(2);
const BUDGET_P99: Duration = Duration::from_millis(10);
const BUDGET_MIB: f64 = 128.0;

fn payload(index: usize) -> serde_json::Value {
    let fields: Vec<String> = (0..FIELDS).map(|f| format!("field-{index}-{f}")).collect();
    serde_json::json!({
        "capability": format!("bench.capability-{index}"),
        "field_refs": fields,
        "policy_digest": Digest256::sha256(b"bench-policy").to_hex(),
    })
}

fn commit(core: &GraphCore, methods: &[Method]) {
    for method in methods {
        match method {
            Method::AddNode {
                node_id,
                properties_msgpack,
            } => core.add_node(node_id.clone(), properties_msgpack.clone()),
            Method::AddEdge {
                source_id,
                target_id,
                properties_msgpack,
            } => core
                .add_edge(
                    source_id.clone(),
                    target_id.clone(),
                    properties_msgpack.clone(),
                )
                .expect("bench edge endpoints exist"),
            Method::RemoveEdge {
                source_id,
                target_id,
            } => core.remove_edge(source_id.clone(), target_id.clone()),
            other => panic!("unexpected method {other:?}"),
        }
    }
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
    let core = Arc::new(GraphCore::new());
    let samples = Arc::new(Mutex::new(Vec::with_capacity(DECLARATIONS)));
    let handles: Vec<_> = (0..WRITERS)
        .map(|writer| {
            let core = Arc::clone(&core);
            let samples = Arc::clone(&samples);
            std::thread::spawn(move || {
                let all = |_: &str| true;
                for index in (writer..DECLARATIONS).step_by(WRITERS) {
                    let payload = payload(index);
                    let view = CoreDeclarationView {
                        core: &core,
                        visible: &all,
                    };
                    let started = Instant::now();
                    let declared = declare_data_prep(&view, "tenant:bench", &payload)
                        .expect("bench declarations are valid");
                    let elapsed = started.elapsed();
                    if let Declared::Write { write_set, .. } = declared {
                        commit(&core, write_set.methods());
                    }
                    samples.lock().expect("samples").push(elapsed);
                }
            })
        })
        .collect();
    for handle in handles {
        handle.join().expect("writer thread");
    }
    let mut samples = samples.lock().expect("samples").clone();
    samples.sort();
    let p50 = samples[samples.len() / 2];
    let p99 = samples[(samples.len() * 99 / 100).min(samples.len() - 1)];
    let peak = high_water_mib();
    println!(
        "ingest/data_prep_declare p50 = {p50:?} (budget {BUDGET_P50:?}) p99 = {p99:?} (budget {BUDGET_P99:?}) peak_mib = {peak:.1} (budget {BUDGET_MIB})"
    );
    assert!(p50 < BUDGET_P50, "p50 {p50:?} exceeds {BUDGET_P50:?}");
    assert!(p99 < BUDGET_P99, "p99 {p99:?} exceeds {BUDGET_P99:?}");
    assert!(
        peak < BUDGET_MIB,
        "peak {peak:.1} MiB exceeds {BUDGET_MIB} MiB"
    );
}
