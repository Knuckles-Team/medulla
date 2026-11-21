//! Served `Discover` planning at one million nodes (MEDULLA-FEDERATED-QUERY-R090.6).
//!
//! Workload: 1,000,000 `Doc` nodes with 8-dimensional embeddings (the width of the hash
//! embedder, so deep-mode expansion vectors share the space) added through the served
//! `AddNode` and `AddEmbedding` methods; every 1,000th node has one edge to the next, so
//! `hops = 2` has neighbours. Criterion group `discover`: `standard_1m` (one weighted blend,
//! no hops) and `deep_overhead` (three expansions fused by rank, `as_of` recency and two hops).
//! The expansion provider answers at once, so the deep time excludes provider latency.
//! Budgets: 15 ms and 40 ms p99 (`latency_p99_ns_max` in
//! `crates/medulla-plan/benches/thresholds.json`). Nightly only; never a PR gate.
//!
//! Run: `cargo bench --features server,security,query --bench discover_planning`

#![allow(dead_code)]

#[path = "../tests/common/mod.rs"]
mod common;
#[path = "../tests/common/test_support.rs"]
mod test_support;

use std::sync::Arc;

use criterion::{criterion_group, criterion_main, Criterion};
use medulla_ingestion_types::ingestion_wire::DiscoverMode;
use medulla_plan::query_understanding::{
    set_expansion_provider, Expansion, ExpansionError, ExpansionKind, ExpansionProvider,
    ExpansionRequest,
};
use medulla_protocol_types::protocol::Method;
use tokio::runtime::Builder;

const SECRET: &str = "discover-planning-bench-secret";
const GRAPH: &str = "__commons__";
const NODES: u64 = 1_000_000;
const DIM: usize = 8;
const SEED: u64 = 0x5EED;

/// The splitmix64 step, the bench suite's deterministic-input convention.
fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

fn vector(state: &mut u64) -> Vec<f32> {
    (0..DIM)
        .map(|_| (splitmix64(state) >> 40) as f32 / (1u64 << 24) as f32)
        .collect()
}

fn node_id(i: u64) -> String {
    format!("n{i:07}")
}

/// Answers three fixed expansions at once.
struct FixedExpansions;

impl ExpansionProvider for FixedExpansions {
    fn expand(&self, req: &ExpansionRequest) -> Result<Vec<Expansion>, ExpansionError> {
        Ok(["a recent document", "a replicated document", "a document about replication"]
            .into_iter()
            .take(usize::from(req.perspectives))
            .enumerate()
            .map(|(i, text)| Expansion {
                text: text.to_string(),
                kind: ExpansionKind::Perspective,
                digest: format!("{i:064x}"),
            })
            .collect())
    }

    fn provider_id(&self) -> String {
        "bench".to_string()
    }

    fn provider_revision(&self) -> String {
        "1".to_string()
    }
}

async fn seeded_state() -> test_support::SharedState {
    common::configure_authority();
    let state = test_support::durable_state(SECRET, common::current_isolation());
    let mut prng = SEED;
    let mut id = 1u64;
    for i in 0..NODES {
        let add = Method::AddNode {
            node_id: node_id(i),
            properties_msgpack: test_support::json_bytes(
                serde_json::json!({ "type": "Doc", "updated_at": i }),
            ),
        };
        test_support::call_ok(&state, SECRET, id, GRAPH, add).await;
        id += 1;
        let embed = Method::AddEmbedding {
            node_id: node_id(i),
            embedding: vector(&mut prng),
        };
        test_support::call_ok(&state, SECRET, id, GRAPH, embed).await;
        id += 1;
        if i % 1_000 == 0 && i + 1 < NODES {
            let edge = Method::AddEdge {
                source_id: node_id(i),
                target_id: node_id(i + 1),
                properties_msgpack: test_support::json_bytes(
                    serde_json::json!({ "relationship": "next" }),
                ),
            };
            test_support::call_ok(&state, SECRET, id, GRAPH, edge).await;
            id += 1;
        }
    }
    state
}

async fn discover(state: &test_support::SharedState, id: u64, method: Method) {
    let response =
        test_support::dispatch(state, test_support::request(SECRET, id, GRAPH, method)).await;
    assert!(response.error.is_none(), "{:?}", response.error);
}

fn standard(query: Vec<f32>) -> Method {
    Method::Discover {
        keywords: vec!["document".to_string()],
        query_embedding: query,
        k: 10,
        quality_policy_ref: None,
        hops: 0,
        query_text: None,
        mode: DiscoverMode::Standard,
        as_of: None,
        perspectives: None,
        capability: None,
    }
}

fn deep(query: Vec<f32>) -> Method {
    Method::Discover {
        keywords: vec!["document".to_string()],
        query_embedding: query,
        k: 10,
        quality_policy_ref: None,
        hops: 2,
        query_text: Some("recent replicated documents".to_string()),
        mode: DiscoverMode::Deep,
        as_of: Some(1_000_000),
        perspectives: Some(3),
        capability: None,
    }
}

// spec: MEDULLA-FEDERATED-QUERY-R090.6
fn discover_planning(c: &mut Criterion) {
    // The deep arms embed their expansions with the hash embedder, set before the first dispatch.
    std::env::set_var("MEDULLA_UQL_TEXT_EMBEDDER", "hash");
    set_expansion_provider(Arc::new(FixedExpansions));
    let rt = Builder::new_multi_thread().enable_all().build().unwrap();
    let state = rt.block_on(seeded_state());
    let mut prng = SEED ^ 0xD15C;
    let query = vector(&mut prng);
    let mut group = c.benchmark_group("discover");
    group.bench_function("standard_1m", |b| {
        b.iter(|| rt.block_on(discover(&state, 10_000_000, standard(query.clone()))));
    });
    group.bench_function("deep_overhead", |b| {
        b.iter(|| rt.block_on(discover(&state, 10_000_001, deep(query.clone()))));
    });
    group.finish();
}

criterion_group! {
    name = benches;
    config = Criterion::default().sample_size(50);
    targets = discover_planning
}
criterion_main!(benches);
