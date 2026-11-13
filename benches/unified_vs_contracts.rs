//! Falsification gate of the unified planner (MEDULLA-FEDERATED-QUERY-R730).
// spec: MEDULLA-FEDERATED-QUERY-R730
//!
//! Workload: `planner.gate.claims` claims with `planner.gate.embedding_dim`-dimension
//! embeddings and `planner.gate.attributes` attributes, under `planner.gate.writers`
//! concurrent writers. The query is a two-hop neighbourhood of the seed claims, an
//! attribute filter and the top `planner.gate.recall_k` vector matches.
//!
//! * Arm A: one served `Method::Uql` plan in one engine.
//! * Arm B: the graph query on one engine and the vector ranking on a second engine, joined
//!   by the client over the public contract.
//!
//! Every number the gate compares is read from the registry knobs `planner.gate.*`. The run
//! writes the report that `tests/test_unified_planner_gate.py` and `scripts/unified_planner_gate.py`
//! judge, to the path in `UNIFIED_VS_CONTRACTS_REPORT` (same convention as `decide_budgets`).
//!
//! Run: `UNIFIED_VS_CONTRACTS_REPORT=target/unified-vs-contracts.json cargo bench --features full --bench unified_vs_contracts`

#[allow(dead_code)]
#[path = "../tests/common/mod.rs"]
mod common;
#[allow(dead_code)]
#[path = "../tests/common/test_support.rs"]
mod test_support;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use medulla_protocol_types::protocol::Method;
use medulla_server_api::knob_config::knobs;
use test_support::SharedState;
use tokio::runtime::Builder;

const SECRET: &str = "unified-vs-contracts-bench-secret"; // sanitizer:ignore
const SEED_STRIDE: usize = 100;
const FANOUT: usize = 3;
const HOT_ATTRIBUTE_MODULUS: usize = 4;
const MEASURED_QUERIES: usize = 30;
const WRITE_LABEL: &str = "GateWrite";

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

fn next_id() -> u64 {
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
}

/// A deterministic unit vector for claim `i` (cosine and dot product agree on unit vectors).
fn embedding(i: usize, dim: usize) -> Vec<f32> {
    let mut state = (i as u64 + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    let mut v: Vec<f32> = (0..dim)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state % 2001) as f32 / 1000.0 - 1.0
        })
        .collect();
    let norm = v
        .iter()
        .map(|x| x * x)
        .sum::<f32>()
        .sqrt()
        .max(f32::MIN_POSITIVE);
    v.iter_mut().for_each(|x| *x /= norm);
    v
}

fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

fn claim_id(i: usize) -> String {
    format!("claim-{i:06}")
}

async fn call(state: &SharedState, method: Method) -> medulla_protocol_types::protocol::Response {
    let response = test_support::dispatch(
        state,
        test_support::commons_request(SECRET, next_id(), method),
    )
    .await;
    assert!(
        response.error.is_none(),
        "gate fixture: {:?}",
        response.error
    );
    response
}

fn fresh_state() -> SharedState {
    test_support::state_with(SECRET, common::current_isolation(), None, None)
}

/// Graph side: claim nodes with attributes and `SUPPORTS` edges.
async fn load_graph(state: &SharedState, claims: usize, attributes: usize) {
    for i in 0..claims {
        let mut props = serde_json::json!({
            "type": "Claim",
            "seed": i % SEED_STRIDE == 0,
        });
        for a in 0..attributes {
            props[format!("attr{a}")] =
                serde_json::json!(if (i + a) % HOT_ATTRIBUTE_MODULUS == 0 {
                    "hot"
                } else {
                    "cold"
                });
        }
        call(
            state,
            Method::AddNode {
                node_id: claim_id(i),
                properties_msgpack: test_support::json_bytes(props),
            },
        )
        .await;
    }
    for i in 0..claims {
        for f in 1..=FANOUT {
            let target = (i * 7 + f * 13) % claims;
            if target != i {
                call(
                    state,
                    Method::AddEdge {
                        source_id: claim_id(i),
                        target_id: claim_id(target),
                        properties_msgpack: test_support::json_bytes(
                            serde_json::json!({"relationship": "SUPPORTS"}),
                        ),
                    },
                )
                .await;
            }
        }
    }
}

/// Vector side: the same claim ids with their embeddings.
async fn load_vectors(state: &SharedState, claims: usize, dim: usize, with_nodes: bool) {
    for i in 0..claims {
        if with_nodes {
            call(
                state,
                Method::AddNode {
                    node_id: claim_id(i),
                    properties_msgpack: test_support::json_bytes(
                        serde_json::json!({"type": "Claim"}),
                    ),
                },
            )
            .await;
        }
        call(
            state,
            Method::AddEmbedding {
                node_id: claim_id(i),
                embedding: embedding(i, dim),
            },
        )
        .await;
    }
}

fn vector_literal(v: &[f32]) -> String {
    let items: Vec<String> = v.iter().map(|x| format!("{x:.6}")).collect();
    format!("[{}]", items.join(", "))
}

fn neighbourhood_uql() -> String {
    "MATCH (:Claim) WHERE seed = TRUE |> TRAVERSE -[:SUPPORTS]-> {1,2} |> WHERE attr0 = 'hot'"
        .to_string()
}

async fn uql_ids(state: &SharedState, text: String) -> Vec<String> {
    let response = call(state, test_support::uql(&text)).await;
    test_support::uql_ids(&response)
}

/// Arm A: one plan, one engine.
async fn arm_a(state: &SharedState, query: &[f32], k: usize) -> Vec<String> {
    let text = format!(
        "{} |> RANK BY ~{} |> LIMIT {k}",
        neighbourhood_uql(),
        vector_literal(query)
    );
    uql_ids(state, text).await
}

/// Arm B: graph query on one engine, vector ranking on the other, client join in vector order.
async fn arm_b(
    graph: &SharedState,
    vectors: &SharedState,
    query: &[f32],
    k: usize,
    claims: usize,
) -> Vec<String> {
    let neighbourhood: BTreeSet<String> =
        uql_ids(graph, format!("{} |> LIMIT {claims}", neighbourhood_uql()))
            .await
            .into_iter()
            .collect();
    let ranked = uql_ids(
        vectors,
        format!(
            "MATCH (:Claim) |> RANK BY ~{} |> LIMIT {claims}",
            vector_literal(query)
        ),
    )
    .await;
    ranked
        .into_iter()
        .filter(|id| neighbourhood.contains(id))
        .take(k)
        .collect()
}

fn percentile(sorted: &[Duration], pct: usize) -> f64 {
    let index = ((sorted.len() * pct).div_ceil(100))
        .saturating_sub(1)
        .min(sorted.len() - 1);
    sorted[index].as_secs_f64() * 1_000.0
}

/// Brute-force reference answer: the neighbourhood is fixed by the generator, so recall is
/// measured against the exact top-k of the neighbourhood by dot product.
fn reference(
    neighbourhood: &BTreeSet<String>,
    query: &[f32],
    dim: usize,
    k: usize,
    claims: usize,
) -> BTreeSet<String> {
    let mut scored: Vec<(f32, String)> = (0..claims)
        .map(claim_id)
        .filter(|id| neighbourhood.contains(id))
        .map(|id| {
            let i: usize = id.trim_start_matches("claim-").parse().expect("claim id");
            (dot(query, &embedding(i, dim)), id)
        })
        .collect();
    scored.sort_by(|a, b| b.0.total_cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    scored.into_iter().take(k).map(|(_, id)| id).collect()
}

/// Run `queries` rounds of `run` while `writers` tasks keep adding nodes; returns the p95
/// latency in milliseconds, the answers and the writes confirmed against the writes attempted.
async fn measured<F, Fut>(
    writer_states: Vec<SharedState>,
    writers: usize,
    writes_each: usize,
    queries: usize,
    mut run: F,
) -> (f64, Vec<Vec<String>>, u64, u64)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Vec<String>>,
{
    let attempted = Arc::new(AtomicU64::new(0));
    let confirmed = Arc::new(AtomicU64::new(0));
    let mut handles = Vec::new();
    for w in 0..writers {
        let states = writer_states.clone();
        let (attempted, confirmed) = (Arc::clone(&attempted), Arc::clone(&confirmed));
        handles.push(tokio::spawn(async move {
            for n in 0..writes_each {
                attempted.fetch_add(1, Ordering::Relaxed);
                let id = format!("gate-write-{w}-{n}");
                let mut ok = true;
                for state in &states {
                    let response = test_support::dispatch(
                        state,
                        test_support::commons_request(
                            SECRET,
                            next_id(),
                            Method::AddNode {
                                node_id: id.clone(),
                                properties_msgpack: test_support::json_bytes(
                                    serde_json::json!({"type": WRITE_LABEL}),
                                ),
                            },
                        ),
                    )
                    .await;
                    ok &= response.error.is_none();
                }
                if ok {
                    confirmed.fetch_add(1, Ordering::Relaxed);
                }
            }
        }));
    }
    let mut latencies = Vec::with_capacity(queries);
    let mut answers = Vec::with_capacity(queries);
    for _ in 0..queries {
        let start = Instant::now();
        answers.push(run().await);
        latencies.push(start.elapsed());
    }
    for h in handles {
        h.await.expect("writer task");
    }
    latencies.sort();
    (
        percentile(&latencies, 95),
        answers,
        attempted.load(Ordering::Relaxed),
        confirmed.load(Ordering::Relaxed),
    )
}

/// Writes visible in `state` after the run, counted through the served read.
async fn visible_writes(state: &SharedState, limit: usize) -> u64 {
    uql_ids(state, format!("MATCH (:{WRITE_LABEL}) |> LIMIT {limit}"))
        .await
        .len() as u64
}

async fn gate() -> BTreeMap<&'static str, serde_json::Value> {
    let k = knobs();
    let claims = k.planner_gate_claims as usize;
    let dim = k.planner_gate_embedding_dim as usize;
    let attributes = k.planner_gate_attributes as usize;
    let writers = k.planner_gate_writers as usize;
    let recall_k = k.planner_gate_recall_k as usize;
    let writes_each = k.planner_gate_writes_per_writer as usize;
    let query = embedding(claims + 1, dim);

    let single = fresh_state();
    load_graph(&single, claims, attributes).await;
    load_vectors(&single, claims, dim, false).await;
    let graph_engine = fresh_state();
    load_graph(&graph_engine, claims, attributes).await;
    let vector_engine = fresh_state();
    load_vectors(&vector_engine, claims, dim, true).await;

    let neighbourhood: BTreeSet<String> = uql_ids(
        &graph_engine,
        format!("{} |> LIMIT {claims}", neighbourhood_uql()),
    )
    .await
    .into_iter()
    .collect();
    let truth = reference(&neighbourhood, &query, dim, recall_k, claims);
    let recall = |answer: &[String]| {
        answer.iter().filter(|id| truth.contains(*id)).count() as f64 / truth.len().max(1) as f64
    };
    let limit = writers * writes_each;

    let (a_p95, a_answers, a_attempted, a_confirmed) = {
        let s = single.clone();
        let q = query.clone();
        measured(
            vec![single.clone()],
            writers,
            writes_each,
            MEASURED_QUERIES,
            move || {
                let (s, q) = (s.clone(), q.clone());
                async move { arm_a(&s, &q, recall_k).await }
            },
        )
        .await
    };
    let a_visible = visible_writes(&single, limit).await;

    let (b_p95, b_answers, b_attempted, b_confirmed) = {
        let (g, v, q) = (graph_engine.clone(), vector_engine.clone(), query.clone());
        measured(
            vec![graph_engine.clone(), vector_engine.clone()],
            writers,
            writes_each,
            MEASURED_QUERIES,
            move || {
                let (g, v, q) = (g.clone(), v.clone(), q.clone());
                async move { arm_b(&g, &v, &q, recall_k, claims).await }
            },
        )
        .await
    };
    let b_visible = visible_writes(&graph_engine, limit)
        .await
        .min(visible_writes(&vector_engine, limit).await);

    let worst = |answers: &[Vec<String>]| {
        answers
            .iter()
            .map(|a| recall(a))
            .fold(f64::INFINITY, f64::min)
    };
    let mut report = BTreeMap::new();
    report.insert("arm_a_p95_ms", serde_json::json!(a_p95));
    report.insert("arm_b_p95_ms", serde_json::json!(b_p95));
    report.insert("arm_a_recall", serde_json::json!(worst(&a_answers)));
    report.insert("arm_b_recall", serde_json::json!(worst(&b_answers)));
    report.insert(
        "writes_attempted",
        serde_json::json!(a_attempted + b_attempted),
    );
    report.insert(
        "writes_lost",
        serde_json::json!(
            (a_attempted - a_visible.min(a_confirmed)) + (b_attempted - b_visible.min(b_confirmed))
        ),
    );
    report.insert(
        "max_p95_ratio",
        serde_json::json!(k.planner_gate_max_p95_ratio),
    );
    report.insert("recall_k", serde_json::json!(recall_k));
    report
}

fn main() {
    let rt = Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .expect("runtime");
    let report = rt.block_on(gate());
    let text = serde_json::to_string_pretty(&report).expect("report json");
    match std::env::var("UNIFIED_VS_CONTRACTS_REPORT") {
        Ok(path) => std::fs::write(path, text).expect("write gate report"),
        Err(_) => println!("{text}"),
    }
}
