//! Served `MultiGraphBatchUpdate` fan-out across eight shard writers
//! (MEDULLA-DURABLE-KERNEL-R099.6).
//!
//! Workload: 64 graphs are seeded through the served method; one graph per
//! shard (eight distinct shard indexes, read from the reply's fan-out reports)
//! is then written in one request of 5,000 nodes each. The nightly run records
//! p50 and p99 of that eight-graph request and asserts p99 stays under 500 ms.
//! It never gates a pull request.
//!
//! Run: `cargo bench --features full --bench multi_graph_fanout`

#![allow(dead_code)]

#[path = "../tests/common/mod.rs"]
mod common;
#[path = "../tests/common/test_support.rs"]
mod test_support;

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use criterion::{criterion_group, criterion_main, Criterion};
use medulla_protocol_types::protocol::{GraphType, Method, ResultPayload};
use medulla_protocol_types::result_contract::transactions::MultiGraphBatchReport;
use serde_json::json;
use tokio::runtime::Builder;

const SECRET: &str = "multi-graph-fanout-bench-secret";
const SHARDS: usize = 8;
const GRAPHS: usize = 64;
const NODES: usize = 5_000;
const SAMPLES: usize = 30;
const P99_BUDGET: Duration = Duration::from_millis(500);

fn blob(graphs: &[String], tag: &str) -> Vec<u8> {
    let batches: Vec<(String, serde_bytes::ByteBuf)> = graphs
        .iter()
        .map(|graph| {
            let ops: Vec<serde_json::Value> = (0..NODES)
                .map(|n| json!({"op": "add_node", "id": format!("{tag}-{n}"), "properties": {"type": "Doc"}}))
                .collect();
            (
                graph.clone(),
                serde_bytes::ByteBuf::from(rmp_serde::to_vec_named(&ops).unwrap()),
            )
        })
        .collect();
    rmp_serde::to_vec_named(&batches).unwrap()
}

async fn write(
    state: &test_support::SharedState,
    id: u64,
    graphs: &[String],
    tag: &str,
) -> MultiGraphBatchReport {
    let response = test_support::dispatch(
        state,
        test_support::commons_request(
            SECRET,
            id,
            Method::MultiGraphBatchUpdate {
                batches_msgpack: blob(graphs, tag),
            },
        ),
    )
    .await;
    assert!(response.error.is_none(), "{:?}", response.error);
    match response.result {
        Some(ResultPayload::Json(value)) => serde_json::from_value(value).unwrap(),
        other => panic!("expected a JSON report, got {other:?}"),
    }
}

fn percentile(sorted: &[Duration], p: f64) -> Duration {
    let idx = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted[idx]
}

// spec: MEDULLA-DURABLE-KERNEL-R099.6
fn multi_graph_fanout(c: &mut Criterion) {
    std::env::set_var("MEDULLA_REDB_SHARDS", SHARDS.to_string());
    common::configure_authority();
    let rt = Builder::new_multi_thread().enable_all().build().unwrap();
    let dir = test_support::fresh_dir("multi-graph-fanout-bench");
    let (_backend, state) = test_support::redb_state_at(
        SECRET,
        common::current_isolation(),
        dir.to_str().expect("utf-8 temp dir"),
    );
    let graphs: Vec<String> = (0..GRAPHS).map(|i| format!("hot:g{i:02}")).collect();
    let picked: Vec<String> = rt.block_on(async {
        for graph in &graphs {
            let s = &mut *state.write().await;
            s.registry
                .create_graph(graph, GraphType::Commons, None)
                .unwrap();
        }
        let seeded = write(&state, 1, &graphs, "seed").await;
        let mut by_shard: BTreeMap<u32, String> = BTreeMap::new();
        for entry in &seeded.fanout {
            by_shard
                .entry(entry.shard_index)
                .or_insert_with(|| entry.graph.clone());
        }
        assert_eq!(by_shard.len(), SHARDS, "every shard must hold a graph");
        by_shard.into_values().collect()
    });

    let mut latencies = Vec::with_capacity(SAMPLES);
    for sample in 0..SAMPLES {
        let tag = format!("run{sample}");
        let started = Instant::now();
        let report = rt.block_on(write(&state, 2 + sample as u64, &picked, &tag));
        latencies.push(started.elapsed());
        assert_eq!(report.fanout.len(), SHARDS);
    }
    latencies.sort();
    let (p50, p99) = (percentile(&latencies, 0.50), percentile(&latencies, 0.99));
    eprintln!("multi_graph_fanout p50={p50:?} p99={p99:?}");
    assert!(p99 < P99_BUDGET, "p99 {p99:?} over {P99_BUDGET:?}");

    c.bench_function("multi_graph_fanout", |b| {
        let mut round = SAMPLES as u64;
        b.iter(|| {
            round += 1;
            rt.block_on(write(
                &state,
                1_000 + round,
                &picked,
                &format!("iter{round}"),
            ))
        });
    });
    std::env::remove_var("MEDULLA_REDB_SHARDS");
}

criterion_group! {
    name = benches;
    config = Criterion::default().sample_size(10);
    targets = multi_graph_fanout
}
criterion_main!(benches);
