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

use medulla_decision_statistical_types::decision::DecisionPolicy;
use medulla_decision_stats::decision::drift::DriftThresholds;
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
            document_set_ids: Vec::new(),
            agent_ref: None,
            seed_ids: Vec::new(),
            hop_filter: None,
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

/// One served `Discover` with an optional policy reference, returned unchecked.
async fn discover_raw(
    state: &test_support::SharedState,
    id: u64,
    policy: Option<&str>,
) -> medulla_protocol_types::protocol::Response {
    let request = common::signed_request(
        SECRET,
        id,
        GRAPH,
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
            document_set_ids: Vec::new(),
            agent_ref: None,
            seed_ids: Vec::new(),
            hop_filter: None,
        },
    );
    Box::pin(dispatch(state, request)).await
}

fn hit_ids(envelope: &serde_json::Value) -> Vec<String> {
    envelope["hits"]
        .as_array()
        .expect("hits")
        .iter()
        .map(|h| h["id"].as_str().expect("hit id").to_string())
        .collect()
}

async fn envelope_under(
    state: &test_support::SharedState,
    id: u64,
    policy: Option<&str>,
) -> serde_json::Value {
    let response = discover_raw(state, id, policy).await;
    assert!(response.error.is_none(), "op {id}: {:?}", response.error);
    match response.result {
        Some(ResultPayload::Json(envelope)) => envelope,
        other => panic!("expected the Discover envelope, got {other:?}"),
    }
}

fn quality_state() -> test_support::SharedState {
    let (persist_dir, persistence) = common::tempdir_persistence();
    test_support::state_with(
        SECRET,
        common::current_isolation(),
        persist_dir,
        persistence,
    )
}

/// Verifies MEDULLA-FEDERATED-QUERY-R089.4: an unknown or wrong-typed policy reference is
/// refused as `INVALID_ARGUMENT`, while a failed verdict is not a refusal and never
/// changes the rows.
// spec: MEDULLA-FEDERATED-QUERY-R089.4
#[tokio::test]
async fn unknown_policy_refuses_and_failed_verdict_keeps_rows() {
    let state = quality_state();
    for n in 0..8 {
        write(&state, n, "known term").await;
    }
    send(
        &state,
        500,
        Method::AddNode {
            node_id: "policy-strict".into(),
            properties_msgpack: pack(json!({
                "type": "RetrievalQualityPolicy",
                "revision": "strict-1",
                "min_composite_quality": 1.0,
                "min_population_ratio": 0.25,
            })),
        },
    )
    .await;
    send(
        &state,
        501,
        Method::AddNode {
            node_id: "not-a-policy".into(),
            properties_msgpack: pack(json!({"type": "Document", "name": "known"})),
        },
    )
    .await;

    for (id, reference) in [(600, "policy-absent"), (601, "not-a-policy")] {
        let refused = discover_raw(&state, id, Some(reference)).await;
        let error = refused.error.expect("an unknown policy is refused");
        assert!(
            format!("{error:?}").contains("INVALID_ARGUMENT"),
            "{reference}: {error:?}"
        );
        assert!(refused.result.is_none(), "{reference} returned rows");
    }

    let default = envelope_under(&state, 602, None).await;
    let strict = envelope_under(&state, 603, Some("policy-strict")).await;
    assert_eq!(strict["verdict"]["gate_passed"], false, "{strict}");
    assert!(!hit_ids(&strict).is_empty());
    assert_eq!(
        hit_ids(&strict),
        hit_ids(&default),
        "rows and order unchanged"
    );
    assert_eq!(strict["hits"], default["hits"], "row content unchanged");
}

/// Verifies MEDULLA-FEDERATED-QUERY-R089.5: the verdict carries the policy revision, the
/// index generation and the thresholds of this call, and Discover writes nothing.
// spec: MEDULLA-FEDERATED-QUERY-R089.5
#[tokio::test]
async fn verdict_carries_revision_generation_and_thresholds() {
    let state = quality_state();
    for n in 0..4 {
        write(&state, n, "known term").await;
    }
    send(
        &state,
        500,
        Method::AddNode {
            node_id: "policy-evidence".into(),
            properties_msgpack: pack(json!({
                "type": "RetrievalQualityPolicy",
                "revision": "evidence-3",
                "min_composite_quality": 0.5,
                "min_population_ratio": 0.75,
            })),
        },
    )
    .await;

    let before = verdict_under(&state, 600, Some("policy-evidence")).await;
    assert_eq!(before["policy_revision"], "evidence-3", "{before}");
    assert_eq!(
        before["thresholds"]["min_composite_quality_q16"],
        (0.5_f64 * 65_535.0).round()
    );
    assert_eq!(
        before["thresholds"]["min_population_ratio_q16"],
        (0.75_f64 * 65_535.0).round()
    );
    let generation = before["index_generation"]
        .as_u64()
        .expect("index_generation");

    // Discover is a read: asking again leaves the generation and the verdict unchanged.
    let again = verdict_under(&state, 601, Some("policy-evidence")).await;
    assert_eq!(again["index_generation"].as_u64(), Some(generation));
    assert_eq!(again["policy_revision"], before["policy_revision"]);
    assert_eq!(again["thresholds"], before["thresholds"]);

    // A write moves the index generation the verdict reports.
    write(&state, 10, "known term").await;
    let after = verdict_under(&state, 602, Some("policy-evidence")).await;
    assert!(
        after["index_generation"]
            .as_u64()
            .expect("index_generation")
            > generation,
        "{before} -> {after}"
    );
}

/// Write one node with no embedding through the commit path.
async fn write_unembedded(state: &test_support::SharedState, n: u64) {
    send(
        state,
        2 * n,
        Method::AddNode {
            node_id: format!("doc-{n}"),
            properties_msgpack: pack(json!({"type": "Document", "name": "known term"})),
        },
    )
    .await;
}

fn modes(verdict: &serde_json::Value) -> Vec<String> {
    verdict["failure_modes"]
        .as_array()
        .expect("failure_modes")
        .iter()
        .map(|m| m.as_str().expect("mode").to_string())
        .collect()
}

/// Verifies MEDULLA-FEDERATED-QUERY-R089.6: an index that embeds a tenth of the graph fails
/// the gate with `sparse_index`; a fully embedded graph does not report it.
// spec: MEDULLA-FEDERATED-QUERY-R089.6
#[tokio::test]
async fn sparse_index_fails_closed() {
    let sparse = quality_state();
    write(&sparse, 0, "known term").await;
    for n in 1..10 {
        write_unembedded(&sparse, n).await;
    }
    let sparse_verdict = verdict(&sparse, 900).await;
    assert_eq!(sparse_verdict["gate_passed"], false, "{sparse_verdict}");
    assert!(
        modes(&sparse_verdict).contains(&"sparse_index".to_string()),
        "{sparse_verdict}"
    );
    let ratio = sparse_verdict["index_population_ratio_q16"]
        .as_u64()
        .expect("ratio");
    assert!(ratio <= 6_554, "a tenth embedded: {ratio}");

    let full = quality_state();
    for n in 0..10 {
        write(&full, n, "known term").await;
    }
    let full_verdict = verdict(&full, 900).await;
    assert!(
        !modes(&full_verdict).contains(&"sparse_index".to_string()),
        "{full_verdict}"
    );
    assert_eq!(full_verdict["index_population_ratio_q16"], 65_535);
}
