//! Served `BusHeartbeat` hot path (MEDULLA-DURABLE-KERNEL-R095.1005).
//!
//! Workload: 10000 heartbeats over 1000 registered sessions on 64 concurrent
//! tasks, through the real `dispatch` entry (`tests/common/test_support.rs`).
//! Budget: p50 under 150 µs, p99 under 900 µs, resident memory under 32 MiB
//! (the nightly gate fails on a regression of more than 10 percent).
//!
//! Run: `cargo bench --features full --bench bus_heartbeat`

#![allow(dead_code)]

#[path = "../tests/common/mod.rs"]
mod common;
#[path = "../tests/common/test_support.rs"]
mod test_support;

use std::sync::Arc;
use std::time::{Duration, Instant};

use criterion::{criterion_group, criterion_main, Criterion};
use medulla_protocol_types::protocol::Method;
use tokio::runtime::Builder;

const SECRET: &str = "bus-heartbeat-bench-secret";
const ENCRYPTION_KEY: &str = "bus-heartbeat-bench-encryption-key-0123456789";
const GRAPH: &str = "busbench";
const SESSIONS: u64 = 1_000;
const HEARTBEATS: u64 = 10_000;
const TASKS: u64 = 64;
const NOW: u64 = 1_800_000_000_000;
const P50_BUDGET: Duration = Duration::from_micros(150);
const P99_BUDGET: Duration = Duration::from_micros(900);
const RSS_BUDGET_BYTES: u64 = 32 * 1024 * 1024;

fn session(i: u64) -> String {
    format!("bench-session-{i:04}")
}

async fn registered_state() -> test_support::SharedState {
    let (_dir, _backend, state) = test_support::durable_graph_state(
        SECRET,
        ENCRYPTION_KEY,
        "bus-heartbeat-bench",
        common::current_isolation(),
        GRAPH,
    )
    .await;
    for i in 0..SESSIONS {
        let method = Method::BusRegister {
            participant: medulla_types::agent_bus::BusParticipant {
                session_ref: session(i),
                tenant: String::new(),
                host_ref: "bench-host".to_string(),
                capabilities: Default::default(),
                last_seen_ms: NOW,
            },
            principal: String::new(),
        };
        test_support::call_ok(&state, SECRET, 1 + i, GRAPH, method).await;
    }
    state
}

/// Run the workload once; returns every heartbeat's latency.
async fn workload(state: &test_support::SharedState) -> Vec<Duration> {
    let per_task = HEARTBEATS / TASKS;
    let mut handles = Vec::new();
    for task in 0..TASKS {
        let state = Arc::clone(state);
        handles.push(tokio::spawn(async move {
            let mut out = Vec::with_capacity(per_task as usize);
            for k in 0..per_task {
                let n = task * per_task + k;
                let method = Method::BusHeartbeat {
                    session_ref: session(n % SESSIONS),
                    now_ms: NOW + n,
                    principal: String::new(),
                };
                let request = test_support::request(SECRET, 100_000 + n, GRAPH, method);
                let start = Instant::now();
                let response = test_support::dispatch(&state, request).await;
                out.push(start.elapsed());
                assert!(response.error.is_none(), "{:?}", response.error);
            }
            out
        }));
    }
    let mut all = Vec::new();
    for handle in handles {
        all.extend(handle.await.expect("heartbeat task"));
    }
    all.sort();
    all
}

fn percentile(sorted: &[Duration], p: f64) -> Duration {
    let idx = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted[idx]
}

fn resident_bytes() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|l| l.starts_with("VmRSS:"))?;
    let kib: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kib * 1024)
}

// spec: MEDULLA-DURABLE-KERNEL-R095.1005
fn bus_heartbeat(c: &mut Criterion) {
    let rt = Builder::new_multi_thread().enable_all().build().unwrap();
    let state = rt.block_on(registered_state());
    let rss_before = resident_bytes().unwrap_or(0);
    let latencies = rt.block_on(workload(&state));
    let rss_growth = resident_bytes().unwrap_or(0).saturating_sub(rss_before);
    let (p50, p99) = (percentile(&latencies, 0.50), percentile(&latencies, 0.99));
    eprintln!("bus_heartbeat p50={p50:?} p99={p99:?} rss_growth={rss_growth}");
    assert!(p50 < P50_BUDGET, "p50 {p50:?} over {P50_BUDGET:?}");
    assert!(p99 < P99_BUDGET, "p99 {p99:?} over {P99_BUDGET:?}");
    assert!(
        rss_growth < RSS_BUDGET_BYTES,
        "resident growth {rss_growth} over budget"
    );

    c.bench_function("bus_heartbeat", |b| {
        b.iter(|| rt.block_on(workload(&state)));
    });
}

criterion_group! {
    name = benches;
    config = Criterion::default().sample_size(10);
    targets = bus_heartbeat
}
criterion_main!(benches);
