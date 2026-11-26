//! MEDULLA-DURABLE-KERNEL-R099.3 / R099.5: a cross-graph write fans out across
//! shard writers over the REAL served `MultiGraphBatchUpdate` dispatch, and the
//! reply carries one `ShardFanoutReport` per sub-batch in request order, and
//! (R099.6) eight graphs on eight distinct shards commit in parallel.
#![cfg(all(feature = "server", feature = "security"))]

mod common;
#[path = "common/test_support.rs"]
mod test_support;

use serde_json::json;

use medulla_protocol_types::mutation_batch::ShardFanoutOutcome;
use medulla_protocol_types::protocol::{GraphType, Method, Response, ResultPayload};
use medulla_protocol_types::result_contract::transactions::MultiGraphBatchReport;
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_server_router::dispatch::request_boundary::{dispatch, dispatch_verified_request};

const SECRET: &str = "shard-write-fanout-secret";
const GRAPHS: [&str; 6] = ["fan:a", "fan:b", "fan:c", "fan:d", "fan:e", "fan:f"];

fn batches_blob(entries: &[(&str, &str)]) -> Vec<u8> {
    let batches: Vec<(String, serde_bytes::ByteBuf)> = entries
        .iter()
        .map(|(graph, node)| {
            let ops = json!([{"op": "add_node", "id": node, "properties": {"type": "Doc"}}]);
            let blob = rmp_serde::to_vec_named(ops.as_array().unwrap()).unwrap();
            (graph.to_string(), serde_bytes::ByteBuf::from(blob))
        })
        .collect();
    rmp_serde::to_vec_named(&batches).unwrap()
}

fn report_of(response: &Response) -> MultiGraphBatchReport {
    assert!(
        response.error.is_none(),
        "dispatch error: {:?}",
        response.error
    );
    match &response.result {
        Some(ResultPayload::Json(value)) => serde_json::from_value(value.clone()).unwrap(),
        other => panic!("expected a JSON report, got {other:?}"),
    }
}

async fn has_node(state: &test_support::SharedState, id: u64, graph: &str, node: &str) -> bool {
    let request = test_support::request(
        SECRET,
        id,
        graph,
        Method::HasNode {
            node_id: node.to_string(),
        },
    );
    let response = Box::pin(dispatch(state, request)).await;
    matches!(response.result, Some(ResultPayload::Bool(true)))
}

async fn durable_state_with_graphs() -> test_support::SharedState {
    let state = test_support::durable_state(SECRET, common::current_isolation());
    for graph in GRAPHS {
        let s = &mut *state.write().await;
        s.registry
            .create_graph(graph, GraphType::Commons, None)
            .unwrap();
    }
    state
}

// spec: MEDULLA-DURABLE-KERNEL-R099.3
// Proves the served arm returns one report per sub-batch and refuses a cross-tenant sub-batch.
#[tokio::test]
async fn served_arm_returns_one_report_per_sub_batch() {
    let state = durable_state_with_graphs().await;
    let entries: Vec<(&str, String)> = GRAPHS
        .iter()
        .enumerate()
        .map(|(i, g)| (*g, format!("n{i}")))
        .collect();
    let refs: Vec<(&str, &str)> = entries.iter().map(|(g, n)| (*g, n.as_str())).collect();
    let response = Box::pin(dispatch(
        &state,
        test_support::commons_request(
            SECRET,
            1,
            Method::MultiGraphBatchUpdate {
                batches_msgpack: batches_blob(&refs),
            },
        ),
    ))
    .await;
    let report = report_of(&response);
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert_eq!(report.fanout.len(), 6, "one report per sub-batch");
    for (index, graph) in GRAPHS.iter().enumerate() {
        assert_eq!(report.fanout[index].graph, *graph, "request order");
        assert_eq!(report.fanout[index].outcome, ShardFanoutOutcome::Committed);
        assert!(report.results.contains_key(*graph));
        assert!(has_node(&state, 10 + index as u64, graph, &format!("n{index}")).await);
    }

    // Cross-tenant: a verified caller of another tenant commits nothing into
    // these graphs and every one of its sub-batches reports a failure.
    let foreign = VerifiedRequestContext::verified_for_test_with_scopes(
        "intruder",
        "tenant-other",
        &["kg:read", "node:write"],
    );
    let request = test_support::commons_request(
        SECRET,
        2,
        Method::MultiGraphBatchUpdate {
            batches_msgpack: batches_blob(&[("fan:a", "stolen")]),
        },
    );
    let refused = Box::pin(dispatch_verified_request(&state, request, foreign)).await;
    if refused.error.is_none() {
        let report = report_of(&refused);
        assert!(
            report
                .fanout
                .iter()
                .all(|entry| matches!(entry.outcome, ShardFanoutOutcome::Failed { .. })),
            "{report:?}"
        );
    }
    assert!(!has_node(&state, 30, "fan:a", "stolen").await);
}

// spec: MEDULLA-DURABLE-KERNEL-R099.5
// Proves the report lists one fanout entry per sub-batch and both shard-writer metrics are scraped.
#[tokio::test]
async fn report_and_metrics_name_every_sub_batch() {
    let state = durable_state_with_graphs().await;
    let refs: Vec<(&str, &str)> = vec![
        ("fan:a", "m0"),
        ("fan:b", "m1"),
        ("fan:c", "m2"),
        ("fan:d", "m3"),
        ("fan:missing", "m4"),
    ];
    let response = Box::pin(dispatch(
        &state,
        test_support::commons_request(
            SECRET,
            1,
            Method::MultiGraphBatchUpdate {
                batches_msgpack: batches_blob(&refs),
            },
        ),
    ))
    .await;
    let report = report_of(&response);
    let graphs: Vec<&str> = report.fanout.iter().map(|r| r.graph.as_str()).collect();
    assert_eq!(
        graphs,
        vec!["fan:a", "fan:b", "fan:c", "fan:d", "fan:missing"]
    );
    assert!(matches!(
        report.fanout[4].outcome,
        ShardFanoutOutcome::Failed { .. }
    ));
    assert!(report.errors.contains_key("fan:missing"));
    for entry in &report.fanout[..4] {
        assert_eq!(entry.outcome, ShardFanoutOutcome::Committed);
    }

    // The serialized report lists `fanout`.
    let json = serde_json::to_value(&report).unwrap();
    assert_eq!(json["fanout"].as_array().map(Vec::len), Some(5));

    #[cfg(feature = "metrics")]
    {
        // Both series name every shard the sub-batches used.
        let rendered = medulla_kernel::metrics::process().render();
        assert!(
            rendered.contains("medulla_shard_writer_inflight"),
            "{rendered}"
        );
        for entry in &report.fanout {
            let label = format!("shard=\"{}\"", entry.shard_index);
            assert!(
                rendered.lines().any(|l| l
                    .starts_with("medulla_shard_writer_commit_seconds_count")
                    && l.contains(&label)),
                "no commit histogram for {label}"
            );
        }
    }
}

/// A request body of `count` sub-batches: the six real graphs first, then
/// graph names that do not exist (they are never reached when the request is
/// refused before the saga).
fn many_batches_blob(count: usize) -> Vec<u8> {
    let ops = json!([{"op": "add_node", "id": "over", "properties": {"type": "Doc"}}]);
    let blob = rmp_serde::to_vec_named(ops.as_array().unwrap()).unwrap();
    let batches: Vec<(String, serde_bytes::ByteBuf)> = (0..count)
        .map(|index| {
            let graph = GRAPHS
                .get(index)
                .map_or_else(|| format!("fan:extra{index}"), |g| (*g).to_string());
            (graph, serde_bytes::ByteBuf::from(blob.clone()))
        })
        .collect();
    rmp_serde::to_vec_named(&batches).unwrap()
}

// spec: MEDULLA-DURABLE-KERNEL-R099.4
// Proves more than 256 sub-batches refuse INVALID_ARGUMENT with zero writes and a failed sub-batch reports its own code.
#[tokio::test]
async fn oversize_refused_and_failed_sub_batch_reported() {
    use medulla_protocol_types::mutation_batch::MAX_FANOUT_SUB_BATCHES;

    let state = durable_state_with_graphs().await;

    // One more than the cap refuses before the saga starts: INVALID_ARGUMENT
    // and not one sub-batch committed, including the six that name real graphs.
    let over = Box::pin(dispatch(
        &state,
        test_support::commons_request(
            SECRET,
            1,
            Method::MultiGraphBatchUpdate {
                batches_msgpack: many_batches_blob(MAX_FANOUT_SUB_BATCHES + 1),
            },
        ),
    ))
    .await;
    assert_eq!(over.error.as_deref(), Some("INVALID_ARGUMENT"), "{over:?}");
    assert!(over.result.is_none());
    for (index, graph) in GRAPHS.iter().enumerate() {
        assert!(
            !has_node(&state, 10 + index as u64, graph, "over").await,
            "{graph} was written by a refused request"
        );
    }

    // A duplicate graph identifier refuses the same way, with nothing written.
    let duplicate = Box::pin(dispatch(
        &state,
        test_support::commons_request(
            SECRET,
            2,
            Method::MultiGraphBatchUpdate {
                batches_msgpack: batches_blob(&[("fan:a", "dup0"), ("fan:a", "dup1")]),
            },
        ),
    ))
    .await;
    assert_eq!(duplicate.error.as_deref(), Some("INVALID_ARGUMENT"));
    assert!(!has_node(&state, 20, "fan:a", "dup0").await);

    // An invalid graph identifier refuses the same way.
    let invalid = Box::pin(dispatch(
        &state,
        test_support::commons_request(
            SECRET,
            3,
            Method::MultiGraphBatchUpdate {
                batches_msgpack: batches_blob(&[("  ", "blank0")]),
            },
        ),
    ))
    .await;
    assert_eq!(invalid.error.as_deref(), Some("INVALID_ARGUMENT"));

    // A failing sub-batch beside committed ones reports its own stable code
    // while the others stay committed.
    let mixed = Box::pin(dispatch(
        &state,
        test_support::commons_request(
            SECRET,
            4,
            Method::MultiGraphBatchUpdate {
                batches_msgpack: batches_blob(&[
                    ("fan:a", "ok0"),
                    ("fan:missing", "bad"),
                    ("fan:b", "ok1"),
                ]),
            },
        ),
    ))
    .await;
    let report = report_of(&mixed);
    assert_eq!(report.fanout.len(), 3);
    assert_eq!(report.fanout[0].outcome, ShardFanoutOutcome::Committed);
    assert_eq!(report.fanout[2].outcome, ShardFanoutOutcome::Committed);
    match &report.fanout[1].outcome {
        ShardFanoutOutcome::Failed { code, message } => {
            assert!(
                !code.is_empty()
                    && code
                        .chars()
                        .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_'),
                "a stable code, got {code}"
            );
            assert!(!message.is_empty());
        }
        other => panic!("the missing graph must fail, got {other:?}"),
    }
    assert!(report.errors.contains_key("fan:missing"));
    assert!(has_node(&state, 30, "fan:a", "ok0").await);
    assert!(has_node(&state, 31, "fan:b", "ok1").await);
    assert!(!has_node(&state, 32, "fan:missing", "bad").await);
}

const HOT_SECRET: &str = "shard-write-fanout-hot-path-secret";
const HOT_SHARDS: usize = 8;
const HOT_GRAPHS: usize = 64;
const HOT_NODES: usize = 5_000;
/// The parallelism rule of the row: the summed commit times of the eight
/// sub-batches exceed this multiple of the request's wall time.
const PARALLELISM_FACTOR_PERCENT: u64 = 150;

fn bulk_batches_blob(graphs: &[String], tag: &str, nodes: usize) -> Vec<u8> {
    let batches: Vec<(String, serde_bytes::ByteBuf)> = graphs
        .iter()
        .map(|graph| {
            let ops: Vec<serde_json::Value> = (0..nodes)
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

// spec: MEDULLA-DURABLE-KERNEL-R099.6
#[cfg(feature = "redb")]
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn cross_graph_writes_fan_across_shard_writers() {
    // The shard count is read once when the backend opens; configure it first.
    std::env::set_var("MEDULLA_REDB_SHARDS", HOT_SHARDS.to_string());
    common::configure_authority();
    let dir = test_support::fresh_dir("r099-6-fanout");
    let (_backend, state) = test_support::redb_state_at(
        HOT_SECRET,
        common::current_isolation(),
        dir.to_str().expect("utf-8 temp dir"),
    );
    let graphs: Vec<String> = (0..HOT_GRAPHS).map(|i| format!("hot:g{i:02}")).collect();
    for graph in &graphs {
        let s = &mut *state.write().await;
        s.registry
            .create_graph(graph, GraphType::Commons, None)
            .unwrap();
    }

    // Write 64 graphs of 5,000 nodes through the served method and group the
    // graphs by the shard index the reports return.
    let seeded = Box::pin(dispatch(
        &state,
        test_support::commons_request(
            HOT_SECRET,
            1,
            Method::MultiGraphBatchUpdate {
                batches_msgpack: bulk_batches_blob(&graphs, "seed", HOT_NODES),
            },
        ),
    ))
    .await;
    let seeded = report_of(&seeded);
    assert_eq!(seeded.fanout.len(), HOT_GRAPHS);
    let mut by_shard: std::collections::BTreeMap<u32, &str> = std::collections::BTreeMap::new();
    for entry in &seeded.fanout {
        assert_eq!(entry.outcome, ShardFanoutOutcome::Committed, "{entry:?}");
        by_shard
            .entry(entry.shard_index)
            .or_insert(entry.graph.as_str());
    }
    assert_eq!(
        by_shard.len(),
        HOT_SHARDS,
        "the 64 graphs must reach every one of the {HOT_SHARDS} shards: {:?}",
        by_shard.keys().collect::<Vec<_>>()
    );

    // Eight graphs on eight distinct shards in one request.
    let picked: Vec<String> = by_shard.values().map(|g| (*g).to_string()).collect();
    let blob = bulk_batches_blob(&picked, "hot", HOT_NODES);
    let started = std::time::Instant::now();
    let response = Box::pin(dispatch(
        &state,
        test_support::commons_request(
            HOT_SECRET,
            2,
            Method::MultiGraphBatchUpdate {
                batches_msgpack: blob,
            },
        ),
    ))
    .await;
    let wall_us = started.elapsed().as_micros() as u64;
    let report = report_of(&response);
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert_eq!(report.fanout.len(), HOT_SHARDS);
    let shards: std::collections::BTreeSet<u32> =
        report.fanout.iter().map(|e| e.shard_index).collect();
    assert_eq!(shards.len(), HOT_SHARDS, "eight distinct shard indexes");
    for entry in &report.fanout {
        assert_eq!(entry.outcome, ShardFanoutOutcome::Committed, "{entry:?}");
    }
    let summed_us: u64 = report.fanout.iter().map(|e| e.commit_us).sum();
    assert!(
        summed_us * 100 > wall_us * PARALLELISM_FACTOR_PERCENT,
        "no parallelism: summed commit {summed_us} us against wall {wall_us} us"
    );
    std::env::remove_var("MEDULLA_REDB_SHARDS");
}
