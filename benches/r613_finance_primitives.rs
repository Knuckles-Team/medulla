//! Execution-kernel latency budget (MEDULLA-FINANCE-PRIMITIVES-R613.6).
//!
//! Workload: a 1000-slice `FinanceExecutionSchedule` solve and a
//! `FinanceRegimeChain` fit over 1_000_000 bars, both through the real signed
//! `dispatch` entry (`tests/common/test_support.rs`). Budgets: schedule p99 under
//! 2 ms, regime fit p99 under 250 ms. Each run records p50, p95, p99 and peak
//! resident memory (`VmHWM` from `/proc`) per group in
//! `target/finance-r613-report.json` (or `FINANCE_R613_REPORT`); `scripts/bench_gate.py`
//! checks that report against `crates/medulla-plan/benches/thresholds.json` and fails a
//! regression above 10 percent.
//!
//! Run: `cargo bench --features full --bench r613_finance_primitives`

#![allow(dead_code)]

#[path = "../tests/common/mod.rs"]
mod common;
#[path = "../tests/common/test_support.rs"]
mod test_support;

use std::time::{Duration, Instant};

use criterion::{criterion_group, criterion_main, Criterion};
use medulla_protocol_types::protocol::{GraphType, Method};
use tokio::runtime::Builder;

const SECRET: &str = "medulla-fin-r613-bench-secret";
const GRAPH: &str = "fin-r613-bench";
const SCHEDULE_SLICES: usize = 1_000;
const REGIME_BARS: usize = 1_000_000;
const SCHEDULE_SAMPLES: usize = 200;
const REGIME_SAMPLES: usize = 20;
const SCHEDULE_P99_BUDGET: Duration = Duration::from_millis(2);
const REGIME_P99_BUDGET: Duration = Duration::from_millis(250);

fn schedule() -> Method {
    Method::FinanceExecutionSchedule {
        symbol: "AAPL".into(),
        total_shares: 1_000.0,
        horizon: 1.0,
        slices: SCHEDULE_SLICES,
        volatility: 1.0,
        risk_aversion: 1.0,
        temporary_impact: 1.0,
        permanent_impact: 0.0,
        continuous: false,
    }
}

fn regime() -> Method {
    Method::FinanceRegimeChain {
        returns: (0..REGIME_BARS)
            .map(|i| [0.02, -0.02, 0.0][i % 3])
            .collect(),
        threshold: 0.01,
        horizon: 1,
    }
}

async fn state() -> test_support::SharedState {
    common::configure_authority();
    let state = test_support::durable_state(SECRET, common::current_isolation());
    let create = Method::CreateGraph {
        graph_name: GRAPH.to_string(),
        graph_type: GraphType::Global,
    };
    let created =
        test_support::dispatch(&state, test_support::request(SECRET, 1, GRAPH, create)).await;
    assert!(
        created.error.is_none(),
        "CreateGraph: {:?}",
        created.error_detail
    );
    state
}

async fn timed(state: &test_support::SharedState, id: u64, method: Method) -> Duration {
    let request = test_support::request(SECRET, id, GRAPH, method);
    let start = Instant::now();
    let response = test_support::dispatch(state, request).await;
    let elapsed = start.elapsed();
    assert!(response.error.is_none(), "{:?}", response.error_detail);
    elapsed
}

fn percentile_ms(sorted: &[Duration], p: f64) -> f64 {
    let idx = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted[idx].as_secs_f64() * 1000.0
}

/// Resident high-water mark in MiB (`VmHWM`), 0 where procfs is absent.
fn high_water_mib() -> f64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| {
            status
                .lines()
                .find(|line| line.starts_with("VmHWM:"))
                .and_then(|line| line.split_whitespace().nth(1))
                .and_then(|kib| kib.parse::<f64>().ok())
        })
        .map_or(0.0, |kib| kib / 1024.0)
}

fn sample(
    rt: &tokio::runtime::Runtime,
    state: &test_support::SharedState,
    method: &Method,
    samples: usize,
) -> serde_json::Value {
    let mut latencies = Vec::with_capacity(samples);
    for n in 0..samples {
        latencies.push(rt.block_on(timed(state, 10 + n as u64, method.clone())));
    }
    latencies.sort();
    serde_json::json!({
        "p50_ms": percentile_ms(&latencies, 0.50),
        "p95_ms": percentile_ms(&latencies, 0.95),
        "p99_ms": percentile_ms(&latencies, 0.99),
        "peak_mib": high_water_mib(),
    })
}

// spec: MEDULLA-FINANCE-PRIMITIVES-R613.6
fn r613_finance_primitives(c: &mut Criterion) {
    let rt = Builder::new_multi_thread().enable_all().build().unwrap();
    let state = rt.block_on(state());
    let schedule_method = schedule();
    let regime_method = regime();

    let schedule_report = sample(&rt, &state, &schedule_method, SCHEDULE_SAMPLES);
    let regime_report = sample(&rt, &state, &regime_method, REGIME_SAMPLES);
    for (name, report, budget) in [
        ("r613/schedule_1000", &schedule_report, SCHEDULE_P99_BUDGET),
        ("r613/regime_1m", &regime_report, REGIME_P99_BUDGET),
    ] {
        let p99 = report["p99_ms"].as_f64().unwrap_or(f64::MAX);
        eprintln!("{name} {report}");
        assert!(
            p99 < budget.as_secs_f64() * 1000.0,
            "{name} p99 {p99} ms over {budget:?}"
        );
    }
    let path = std::env::var("FINANCE_R613_REPORT")
        .unwrap_or_else(|_| "target/finance-r613-report.json".to_string());
    let body = serde_json::json!({
        "groups": {
            "r613/schedule_1000": schedule_report,
            "r613/regime_1m": regime_report,
        }
    });
    std::fs::write(&path, serde_json::to_vec_pretty(&body).unwrap())
        .expect("write r613 budget report");

    let mut group = c.benchmark_group("r613");
    group.bench_function("schedule_1000", |b| {
        b.iter(|| rt.block_on(timed(&state, 5_000, schedule_method.clone())));
    });
    group.bench_function("regime_1m", |b| {
        b.iter(|| rt.block_on(timed(&state, 5_001, regime_method.clone())));
    });
    group.finish();
}

criterion_group! {
    name = benches;
    config = Criterion::default().sample_size(10);
    targets = r613_finance_primitives
}
criterion_main!(benches);
