//! The served spec intelligence report at 20,000 rows (MEDULLA-SDLC-R014.13).
//!
//! Workload: 20,000 `Requirement` rows spread over eight repositories, each with one bound
//! test, a verified-by edge and two of 500 shared glossary terms, plus one `dependsOnRow` edge
//! to the previous row of the same repository. They go in through the served `BatchUpdate`
//! method, nodes ahead of edges. The report is then read through the served `UnifiedQuery`
//! method carrying `spec_report` and an empty plan, the one entry the row defines; the bench
//! adds no seam of its own. Criterion group `spec_intelligence`: `report_20k`.
//! Budget: 20 s p99 (`spec_intelligence/report_20k` in
//! `crates/medulla-plan/benches/thresholds.json`). Nightly only; never a PR gate.
//!
//! Run: `cargo bench --features server,security,query,shacl --bench spec_intelligence`

#![allow(dead_code)]

#[path = "../tests/common/mod.rs"]
mod common;
#[path = "../tests/common/test_support.rs"]
mod test_support;

use criterion::{criterion_group, criterion_main, Criterion};
use medulla_protocol_types::protocol::Method;
use medulla_types::spec_report::SpecReportQuery;
use serde_json::{json, Value};
use tokio::runtime::Builder;

const SECRET: &str = "spec-intelligence-bench-secret";
const GRAPH: &str = "__commons__";
const ROWS: usize = 20_000;
const REPOSITORIES: usize = 8;
const TERMS: usize = 500;
const CHUNK: usize = 5_000;

fn repo_of(row: usize) -> String {
    format!("repo{}", row % REPOSITORIES)
}

fn row_id(row: usize) -> String {
    format!("BENCH{}-R{:05}", row % REPOSITORIES, row / REPOSITORIES + 1)
}

fn requirement(row: usize) -> String {
    format!("requirement:{}:{}", repo_of(row), row_id(row))
}

fn node(id: &str, properties: Value) -> Value {
    json!({"op": "upsert_node", "id": id, "properties": properties})
}

fn edge(source: &str, relationship: &str, target: &str) -> Value {
    json!({
        "op": "upsert_edge",
        "source": source,
        "target": target,
        "properties": {"relationship": relationship},
    })
}

/// Every node, then every edge, so no edge names a node a later write adds.
fn operations() -> Vec<Value> {
    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    for term in 0..TERMS {
        nodes.push(node(
            &format!("term:bench_term_{term}"),
            json!({"type": "GlossaryTerm", "name": format!("bench_term_{term}")}),
        ));
    }
    for row in 0..ROWS {
        let id = requirement(row);
        let test = format!("testfile:{}:tests/test_{row}.py", repo_of(row));
        nodes.push(node(
            &id,
            json!({
                "type": "Requirement",
                "repo": repo_of(row),
                "rowId": row_id(row),
                "rowStatus": "SPECIFIED",
            }),
        ));
        nodes.push(node(
            &test,
            json!({"type": "TestFile", "repo": repo_of(row), "path": format!("tests/test_{row}.py")}),
        ));
        edges.push(edge(&id, "boundTest", &test));
        edges.push(edge(&id, "verifiedBy", &test));
        for offset in [0, 7] {
            let term = (row * 13 + offset) % TERMS;
            edges.push(edge(&id, "usesTerm", &format!("term:bench_term_{term}")));
        }
        if row >= REPOSITORIES {
            edges.push(edge(&id, "dependsOnRow", &requirement(row - REPOSITORIES)));
        }
    }
    nodes.extend(edges);
    nodes
}

async fn seeded_state() -> test_support::SharedState {
    common::configure_authority();
    let state = test_support::durable_state(SECRET, common::current_isolation());
    for (n, chunk) in operations().chunks(CHUNK).enumerate() {
        let batch = Method::BatchUpdate {
            operations_msgpack: test_support::json_bytes(Value::Array(chunk.to_vec())),
        };
        test_support::call_ok(&state, SECRET, 1 + n as u64, GRAPH, batch).await;
    }
    state
}

/// One served report: `UnifiedQuery` with an empty plan and the `spec_report` body.
async fn report(state: &test_support::SharedState, id: u64) {
    let method = Method::UnifiedQuery {
        plan: medulla_query_types::wire::Plan::new(Vec::new()),
        foreign_execution: None,
        foreign_executions: Default::default(),
        code_map: None,
        spec_report: Some(Box::new(SpecReportQuery::default())),
        retrieval: None,
        latency_class: Default::default(),
    };
    let response =
        test_support::dispatch(state, test_support::request(SECRET, id, GRAPH, method)).await;
    assert!(response.error.is_none(), "{:?}", response.error);
}

// spec: MEDULLA-SDLC-R014.13
fn spec_intelligence(c: &mut Criterion) {
    let rt = Builder::new_multi_thread().enable_all().build().unwrap();
    let state = rt.block_on(seeded_state());
    let mut group = c.benchmark_group("spec_intelligence");
    group.bench_function("report_20k", |b| {
        b.iter(|| rt.block_on(report(&state, 1_000_000)));
    });
    group.finish();
}

criterion_group! {
    name = benches;
    config = Criterion::default().sample_size(10);
    targets = spec_intelligence
}
criterion_main!(benches);
