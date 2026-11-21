//! Served `NlQuery` engine overhead per planner step (MEDULLA-FEDERATED-QUERY-R058.4.6).
//!
//! Workload: a scripted planner answers at once, so the measured time is the engine's own
//! overhead (grounding, the deterministic UQL pipeline and the correction round trip) and
//! never planner latency, through the real `dispatch` entry (`tests/common/test_support.rs`)
//! over 1,000 `Doc` nodes. Criterion group `nl_steps`: `one_step` and `corrected_two_steps`.
//! Budget: 10 ms p99 per step (`latency_p99_ns_max` in
//! `crates/medulla-plan/benches/thresholds.json`). Nightly only; never a PR gate.
//!
//! Run: `cargo bench --features server,security,nl-query --bench nl_steps`

#![allow(dead_code)]

#[path = "../tests/common/mod.rs"]
mod common;
#[path = "../tests/common/test_support.rs"]
mod test_support;

use std::sync::Arc;

use criterion::{criterion_group, criterion_main, Criterion};
use medulla_protocol_types::protocol::Method;
use medulla_query_types::nl_query::NlDialect;
use tokio::runtime::Builder;

const SECRET: &str = "nl-steps-bench-secret";
const GRAPH: &str = "__commons__";
const DOCS: u64 = 1_000;
const DIRECT: &str = "direct";
const CORRECTED: &str = "corrected";

/// Answers `direct` with a valid statement at once, and `corrected` with an invalid one until
/// the engine has sent the typed refusal back, then with the valid one.
struct BenchPlanner;

impl medulla_nlq::nl::NlPlanner for BenchPlanner {
    fn id(&self) -> String {
        "bench".to_string()
    }

    fn revision(&self) -> String {
        "1".to_string()
    }

    fn plan(&self, nl: &str, schema_hint: &str) -> Result<String, String> {
        if nl == CORRECTED && !schema_hint.contains("A previous answer was refused") {
            return Ok("MATCH (:Doc".to_string());
        }
        Ok("MATCH (:Doc) |> LIMIT 10".to_string())
    }
}

async fn seeded_state() -> test_support::SharedState {
    common::configure_authority();
    let state = test_support::durable_state(SECRET, common::current_isolation());
    for i in 0..DOCS {
        let method = Method::AddNode {
            node_id: format!("doc-{i:04}"),
            properties_msgpack: test_support::json_bytes(
                serde_json::json!({ "type": "Doc", "text": format!("entry {i}") }),
            ),
        };
        test_support::call_ok(&state, SECRET, 1 + i, GRAPH, method).await;
    }
    state
}

async fn ask(state: &test_support::SharedState, id: u64, text: &str) {
    let method = Method::NlQuery {
        text: text.to_string(),
        graph: GRAPH.to_string(),
        history: vec![],
        dialect: NlDialect::Auto,
        max_steps: None,
    };
    let response = test_support::dispatch(state, test_support::request(SECRET, id, GRAPH, method))
        .await;
    assert!(response.error.is_none(), "{:?}", response.error);
}

// spec: MEDULLA-FEDERATED-QUERY-R058.4.6
fn nl_steps(c: &mut Criterion) {
    medulla_h_query::nl::set_nl_planner(Arc::new(BenchPlanner));
    let rt = Builder::new_multi_thread().enable_all().build().unwrap();
    let state = rt.block_on(seeded_state());
    let mut group = c.benchmark_group("nl_steps");
    group.bench_function("one_step", |b| {
        b.iter(|| rt.block_on(ask(&state, 100_000, DIRECT)));
    });
    group.bench_function("corrected_two_steps", |b| {
        b.iter(|| rt.block_on(ask(&state, 100_001, CORRECTED)));
    });
    group.finish();
}

criterion_group! {
    name = benches;
    config = Criterion::default().sample_size(50);
    targets = nl_steps
}
criterion_main!(benches);
