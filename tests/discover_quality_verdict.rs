//! The `Discover` retrieval-quality verdict, served (MEDULLA-FEDERATED-QUERY-R089.7).
//!
//! Drift mode: a firing drift monitor of MEDULLA-DECISION-ENGINE-R174.8 makes the served
//! `Discover` verdict carry the `drift` failure mode, the generation of each monitored shard
//! and the adapter digest (`base` when none is active). Every write goes through the real
//! dispatch path, so the monitors are fed by the commit path and not by the test.

mod common;
#[path = "common/test_support.rs"]
mod test_support;

use serde_json::json;

use medulla_decision_stats::decision::drift::DriftThresholds;
use medulla_decision_types::decision::DecisionPolicy;
use medulla_protocol_types::protocol::{Method, ResultPayload};
use medulla_server_router::dispatch::request_boundary::dispatch;

const SECRET: &str = "discover-quality-verdict-secret";
const GRAPH: &str = "__commons__";
/// The reference window of the default drift policy.
const WINDOW: u64 = 64;

fn pack(v: serde_json::Value) -> Vec<u8> {
    rmp_serde::to_vec_named(&v).unwrap()
}

async fn send(state: &test_support::SharedState, id: u64, method: Method) -> ResultPayload {
    let request = common::signed_request(SECRET, id, GRAPH, method);
    let response = Box::pin(dispatch(state, request)).await;
    assert!(response.error.is_none(), "op {id}: {:?}", response.error);
    response.result.expect("a result")
}

/// Write one node and its embedding through the commit path.
async fn write(state: &test_support::SharedState, n: u64, name: &str) {
    let node_id = format!("doc-{n}");
    send(
        state,
        2 * n,
        Method::AddNode {
            node_id: node_id.clone(),
            properties_msgpack: pack(json!({"type": "Document", "name": name})),
        },
    )
    .await;
    let seed = (n % 7) as f32;
    send(
        state,
        2 * n + 1,
        Method::AddEmbedding {
            node_id,
            embedding: vec![seed, 1.0, 2.0],
        },
    )
    .await;
}

async fn verdict(state: &test_support::SharedState, id: u64) -> serde_json::Value {
    verdict_under(state, id, None).await
}

async fn verdict_under(
    state: &test_support::SharedState,
    id: u64,
    policy: Option<&str>,
) -> serde_json::Value {
    let result = send(
        state,
        id,
        Method::Discover {
            keywords: vec!["known".into()],
            query_embedding: vec![1.0, 1.0, 2.0],
            k: 5,
            quality_policy_ref: policy.map(str::to_string),
            hops: 0,
            query_text: None,
            mode: Default::default(),
            as_of: None,
            perspectives: None,
            capability: None,
        },
    )
    .await;
    match result {
        ResultPayload::Json(serde_json::Value::Object(envelope)) => envelope["verdict"].clone(),
        other => panic!("expected the Discover envelope, got {other:?}"),
    }
}

/// Verifies MEDULLA-FEDERATED-QUERY-R089.3: a `RetrievalQualityPolicy` node named by
/// `quality_policy_ref` replaces the knob thresholds and its revision rides the verdict.
// spec: MEDULLA-FEDERATED-QUERY-R089.3
#[tokio::test]
async fn policy_node_overrides_defaults() {
    let (persist_dir, persistence) = common::tempdir_persistence();
    let state = test_support::state_with(
        SECRET,
        common::current_isolation(),
        persist_dir,
        persistence,
    );
    for n in 0..8 {
        write(&state, n, "known term").await;
    }
    // A policy no answer can clear: the quality floor is the top of the scale.
    send(
        &state,
        500,
        Method::AddNode {
            node_id: "policy-strict".into(),
            properties_msgpack: pack(json!({
                "type": "RetrievalQualityPolicy",
                "revision": "strict-7",
                "min_composite_quality": 1.0,
                "min_population_ratio": 0.25,
            })),
        },
    )
    .await;

    let default = verdict_under(&state, 600, None).await;
    assert_eq!(default["policy_revision"], "default", "{default}");
    let knobs = medulla_server_api::knob_config::knobs();
    assert_eq!(
        default["thresholds"]["min_composite_quality_q16"],
        (knobs.discover_min_composite_quality.clamp(0.0, 1.0) * 65_535.0).round()
    );

    let strict = verdict_under(&state, 601, Some("policy-strict")).await;
    assert_eq!(strict["policy_revision"], "strict-7", "{strict}");
    assert_eq!(strict["thresholds"]["min_composite_quality_q16"], 65_535);
    assert_eq!(
        strict["thresholds"]["min_population_ratio_q16"],
        (0.25_f64 * 65_535.0).round()
    );
    assert_eq!(strict["above_threshold"], false, "{strict}");
    assert!(strict["failure_modes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|m| m == "low_relevance_topk"));
}

/// Verifies MEDULLA-FEDERATED-QUERY-R089.7: Behavior: drift mode.
// spec: MEDULLA-FEDERATED-QUERY-R089.7
#[tokio::test]
async fn drift_mode_names_adapter_and_shard_generations() {
    let (persist_dir, persistence) = common::tempdir_persistence();
    let state = test_support::state_with(
        SECRET,
        common::current_isolation(),
        persist_dir,
        persistence,
    );

    // The reference window: in-vocabulary writes seal the shard's reference.
    for n in 0..WINDOW {
        write(&state, n, "known term").await;
    }
    let calm = verdict(&state, 10_000).await;
    assert!(!calm["failure_modes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|m| m == "drift"));
    assert_eq!(calm["adapter_digest"], "base");

    // Writes carrying tokens the reference never saw, evaluated twice: the monitor fires.
    for n in WINDOW..2 * WINDOW {
        write(&state, n, &format!("unseen-{n} vocabulary")).await;
    }
    let tenant = medulla_server_api::carrier::tenant_scope_of(common::TEST_TENANT);
    let thresholds = DriftThresholds::from_policy(&DecisionPolicy::engine_default());
    let registry = medulla_h_decide::decide::drift_registry::registry();
    for _ in 0..2 {
        registry.evaluate(&tenant, GRAPH, &thresholds);
    }

    let drifted = verdict(&state, 10_001).await;
    let modes = drifted["failure_modes"].as_array().unwrap();
    assert!(modes.iter().any(|m| m == "drift"), "{drifted}");
    assert_eq!(drifted["gate_passed"], false);
    assert_eq!(drifted["adapter_digest"], "base");
    let shards = drifted["shard_generations"].as_array().unwrap();
    assert_eq!(shards.len(), 1, "{drifted}");
    assert_eq!(shards[0]["generation"], 1);
}
