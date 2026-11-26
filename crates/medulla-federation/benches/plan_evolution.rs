//! MEDULLA-FEDERATED-QUERY-R091.6: the hot-path cost of recording one fragment observation
//! for plan evolution. Every served foreign fragment pays it, so its p50 must stay under the
//! `plan_evolution_record` key of `thresholds.json` (20 microseconds). Reported, never a PR gate.
//!
//! Run: `cargo bench -p medulla-federation --features federation --bench plan_evolution`
// spec: MEDULLA-FEDERATED-QUERY-R091.6

use std::time::Instant;

use medulla_catalog_types::metadata_acceleration::FragmentExecutionMode;
use medulla_federation::federation_opt::plan_evolution::{observe, ReplayInput};
use medulla_federation::federation_opt::{FetchStrategy, FragmentTrace};

const RUNS: usize = 10_000;

fn ceiling_ns() -> u64 {
    let thresholds: serde_json::Value = serde_json::from_str(include_str!("thresholds.json"))
        .expect("thresholds.json is valid JSON");
    thresholds["plan_evolution_record"]
        .as_u64()
        .expect("thresholds.json names plan_evolution_record")
}

fn main() {
    let mut trace = FragmentTrace::new(
        "sql:bench#00000000".to_string(),
        FetchStrategy::BindJoin,
        FragmentExecutionMode::Live,
    );
    trace.keys_pushed = 8;
    trace.limit_pushed = Some(10);
    trace.rows_fetched = 50;
    trace.elapsed_ms = 3;
    let mut samples = Vec::with_capacity(RUNS);
    for _ in 0..RUNS {
        let start = Instant::now();
        observe(
            "bench-tenant",
            std::hint::black_box(&trace),
            ReplayInput::default(),
        );
        samples.push(start.elapsed().as_nanos() as u64);
    }
    samples.sort_unstable();
    let p50 = samples[RUNS / 2];
    let p99 = samples[RUNS * 99 / 100];
    println!("plan_evolution_record p50_ns={p50} p99_ns={p99}");
    let ceiling = ceiling_ns();
    assert!(
        p50 <= ceiling,
        "plan_evolution_record p50 {p50} ns exceeds the {ceiling} ns ceiling"
    );
}
