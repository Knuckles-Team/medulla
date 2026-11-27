//! MEDULLA-FEDERATED-QUERY-R006.1: served `RunTemporalRules` through the real
//! `dispatch` shell over a redb-backed `ServerState`.
//!
//! Premise edges carry sentence and axiom origins. Matching intervals derive a
//! `DerivedFact` node and nonmatching ones do not; chains derive in event-time
//! order; the reply carries the trace; every refusal leaves the graph version
//! unchanged; and the stored node reads back after a restart.

#![cfg(all(feature = "redb", feature = "reasoning", feature = "query"))]

mod common;
#[path = "common/test_support.rs"]
mod test_support;

use medulla_protocol_types::protocol::{Method, Response, ResultPayload};
use medulla_query_types::derived_fact::{OriginKind, RunTemporalRulesResult};

const SECRET: &str = "forward-chain-served-secret";
const ENCRYPTION_KEY: &str = "forward-chain-served-encryption-key-0123456789";
const GRAPH: &str = "forwardchain";

async fn send(state: &test_support::SharedState, id: u64, method: Method) -> Response {
    test_support::dispatch(state, test_support::request(SECRET, id, GRAPH, method)).await
}

fn msgpack(value: serde_json::Value) -> Vec<u8> {
    rmp_serde::to_vec_named(&value).expect("msgpack")
}

async fn node(state: &test_support::SharedState, id: u64, node_id: &str) {
    let response = send(
        state,
        id,
        Method::AddNode {
            node_id: node_id.to_string(),
            properties_msgpack: msgpack(serde_json::json!({ "type": "Thing" })),
        },
    )
    .await;
    assert!(response.error.is_none(), "{:?}", response.error);
}

#[allow(clippy::too_many_arguments)]
async fn edge(
    state: &test_support::SharedState,
    id: u64,
    source: &str,
    target: &str,
    relationship: &str,
    window: (i64, i64),
    origin: Option<(&str, &str)>,
) {
    let mut props = serde_json::json!({
        "relationship": relationship,
        "valid_from": window.0,
        "valid_until": window.1,
    });
    if let Some((kind, reference)) = origin {
        props["origin_kind"] = serde_json::json!(kind);
        props["origin_ref"] = serde_json::json!(reference);
    }
    let response = send(
        state,
        id,
        Method::AddEdge {
            source_id: source.to_string(),
            target_id: target.to_string(),
            properties_msgpack: msgpack(props),
        },
    )
    .await;
    assert!(response.error.is_none(), "{:?}", response.error);
}

fn rules(text: &str) -> Method {
    Method::RunTemporalRules {
        text: text.to_string(),
    }
}

fn decode(response: Response) -> RunTemporalRulesResult {
    assert!(
        response.error.is_none(),
        "RunTemporalRules failed: {:?} {:?}",
        response.error,
        response.error_detail
    );
    match response.result {
        Some(ResultPayload::Json(value)) => serde_json::from_value(value).expect("typed reply"),
        Some(ResultPayload::Raw(bytes)) => rmp_serde::from_slice(&bytes).expect("typed reply"),
        other => panic!("unexpected payload {other:?}"),
    }
}

fn refusal_names(response: &Response, code: &str) -> bool {
    response
        .error
        .as_deref()
        .is_some_and(|error| error.contains(code))
        || response
            .error_detail
            .as_deref()
            .is_some_and(|detail| detail.contains(code))
}

fn version(state: &test_support::SharedState) -> u64 {
    state
        .try_read()
        .unwrap()
        .registry
        .get(GRAPH)
        .unwrap()
        .core
        .version()
}

async fn stored_props(
    state: &test_support::SharedState,
    id: u64,
    node_id: &str,
) -> serde_json::Map<String, serde_json::Value> {
    let response = send(
        state,
        id,
        Method::GetNodeProperties {
            node_id: node_id.to_string(),
        },
    )
    .await;
    assert!(response.error.is_none(), "{:?}", response.error);
    match response.result {
        Some(ResultPayload::Raw(bytes)) => {
            medulla_types::msgpack::decode_property_object(&bytes).expect("node properties")
        }
        other => panic!("DerivedFact node is missing: {other:?}"),
    }
}

// spec: MEDULLA-FEDERATED-QUERY-R006.1
#[tokio::test(flavor = "multi_thread")]
async fn chain_covers_model_order_origin_refusal_restart_and_trace() {
    let (dir, backend, state) = test_support::durable_graph_state(
        SECRET,
        ENCRYPTION_KEY,
        "forward-chain-served",
        common::current_isolation(),
        GRAPH,
    )
    .await;
    let dir_s = dir.to_string_lossy().to_string();

    for (id, name) in [
        "s1", "s2", "s3", "s4", "x1", "x2", "x3", "x4", "a0", "g1", "g2", "m1", "b1", "d1",
    ]
    .into_iter()
    .enumerate()
    {
        node(&state, 10 + id as u64, name).await;
    }
    // Matching pair: [0,10) and [5,20) overlap on [5,10).
    edge(
        &state,
        40,
        "s1",
        "x1",
        "present",
        (0, 10),
        Some(("sentence", "sent:1")),
    )
    .await;
    edge(
        &state,
        41,
        "s1",
        "x2",
        "assigned",
        (5, 20),
        Some(("axiom", "ax:1")),
    )
    .await;
    // Nonmatching pair: closed-open [0,5) and [5,20) share no instant.
    edge(
        &state,
        42,
        "s2",
        "x3",
        "present",
        (0, 5),
        Some(("sentence", "sent:2")),
    )
    .await;
    edge(
        &state,
        43,
        "s2",
        "x4",
        "assigned",
        (5, 20),
        Some(("axiom", "ax:2")),
    )
    .await;
    // Chain: active [0,10) with grants [0,6) and [4,10).
    edge(
        &state,
        44,
        "s3",
        "a0",
        "active",
        (0, 10),
        Some(("sentence", "sent:3")),
    )
    .await;
    edge(
        &state,
        45,
        "s3",
        "g1",
        "grant",
        (0, 6),
        Some(("sentence", "sent:4")),
    )
    .await;
    edge(
        &state,
        46,
        "s3",
        "g2",
        "grant",
        (4, 10),
        Some(("axiom", "ax:3")),
    )
    .await;
    // Refusal fixtures: no origin, an empty window, two windows on one edge id.
    edge(&state, 47, "s4", "m1", "noorigin", (0, 10), None).await;
    edge(
        &state,
        48,
        "s4",
        "b1",
        "badbound",
        (10, 10),
        Some(("sentence", "sent:5")),
    )
    .await;
    edge(
        &state,
        49,
        "s4",
        "d1",
        "dup",
        (0, 10),
        Some(("sentence", "sent:6")),
    )
    .await;
    edge(
        &state,
        50,
        "s4",
        "d1",
        "dup",
        (0, 11),
        Some(("sentence", "sent:6")),
    )
    .await;

    // Matching intervals derive one fact with its origin; nonmatching derive none.
    let generation = version(&state);
    let colocated = decode(
        send(
            &state,
            60,
            rules("RULE temporal-rules-v1 colocated <- present & assigned"),
        )
        .await,
    );
    assert_eq!(colocated.graph_generation, generation);
    assert!(colocated.committed_version > generation);
    assert_eq!(colocated.facts.len(), 1, "{:?}", colocated.facts);
    let fact = &colocated.facts[0];
    assert_eq!((fact.valid_from, fact.valid_to), (5, 10));
    assert_eq!(fact.fact.premise_ids, ["s1->x1", "s1->x2"]);
    assert_eq!(fact.fact.origin_kind, OriginKind::Sentence);
    assert_eq!(fact.fact.origin_ref, "sent:1");
    assert_eq!(fact.fact.rule_digest, colocated.rule_digest);
    assert_eq!((fact.revision, fact.tombstone), (1, false));
    assert_eq!(colocated.trace.len(), 1);
    assert_eq!(colocated.trace[0].fact_id, fact.fact.fact_id);
    assert_eq!(colocated.trace[0].premise_ids, fact.fact.premise_ids);

    // Chains derive in event-time order and reach a fixpoint.
    let chain = decode(
        send(
            &state,
            61,
            rules("RULE temporal-rules-v1 active <- active & grant"),
        )
        .await,
    );
    let mut windows: Vec<(i64, i64)> = chain
        .facts
        .iter()
        .map(|record| (record.valid_from, record.valid_to))
        .collect();
    assert!(windows.windows(2).all(|pair| pair[0].0 <= pair[1].0));
    windows.sort();
    assert_eq!(windows, [(0, 6), (4, 6), (4, 10)]);
    assert_eq!(chain.trace.iter().map(|step| step.round).max(), Some(2));
    assert_eq!(
        chain
            .trace
            .iter()
            .map(|step| step.order)
            .collect::<Vec<_>>(),
        [0, 1, 2]
    );

    // A second run of the same rule leaves the stored fact unchanged.
    let fact_node = format!("derived:{}", fact.fact.fact_id);
    let first_props = stored_props(&state, 62, &fact_node).await;
    let again = decode(
        send(
            &state,
            63,
            rules("RULE temporal-rules-v1 colocated <- present & assigned"),
        )
        .await,
    );
    assert_eq!(again.facts, colocated.facts);
    assert_eq!(stored_props(&state, 64, &fact_node).await, first_props);

    // Every refusal names its code and leaves the graph version unchanged.
    for (id, text, code) in [
        (
            70,
            "RULE temporal-rules-v2 h <- present",
            "RULE_VERSION_UNSUPPORTED",
        ),
        (
            71,
            "RULE temporal-rules-v1 h <- noorigin",
            "DERIVED_ORIGIN_MISSING",
        ),
        (
            72,
            "RULE temporal-rules-v1 h <- badbound",
            "DERIVED_BOUND_MALFORMED",
        ),
        (
            73,
            "RULE temporal-rules-v1 h <- dup",
            "DERIVED_WINDOW_CONFLICT",
        ),
    ] {
        let before = version(&state);
        let refused = send(&state, id, rules(text)).await;
        assert!(refusal_names(&refused, code), "{code}: {refused:?}");
        assert_eq!(version(&state), before, "{code} changed the graph");
    }
    // A caller without write access on the graph writes nothing.
    let before = version(&state);
    let unauthorized = test_support::dispatch(
        &state,
        common::signed_request_with_context(
            SECRET,
            74,
            GRAPH,
            common::context_for("root"),
            rules("RULE temporal-rules-v1 colocated <- present & assigned"),
        ),
    )
    .await;
    assert!(unauthorized.error.is_some(), "{unauthorized:?}");
    assert_eq!(version(&state), before, "an unauthorized run wrote");

    // The DerivedFact node reads back after a restart.
    state.write().await.persistence = None;
    drop(state);
    let reopened = test_support::reopen_after_sole_reference(
        backend,
        || test_support::open_redb_backend(dir_s.clone()),
        "reopen forward chain backend",
    )
    .await;
    let restarted = test_support::state_with(
        SECRET,
        common::current_isolation(),
        Some(dir_s.clone()),
        Some(reopened.clone()),
    );
    reopened
        .load_all(&restarted)
        .await
        .expect("reload durable graphs");
    let props = stored_props(&restarted, 80, &fact_node).await;
    assert_eq!(
        props.get("type").and_then(|v| v.as_str()),
        Some("DerivedFact")
    );
    assert_eq!(
        props.get("fact_id").and_then(|v| v.as_str()),
        Some(fact.fact.fact_id.as_str())
    );
    assert_eq!(
        props.get("rule_digest").and_then(|v| v.as_str()),
        Some(colocated.rule_digest.to_hex().as_str())
    );
    assert_eq!(
        props.get("graph_generation").and_then(|v| v.as_u64()),
        Some(generation)
    );
    assert_eq!(
        props.get("origin_kind").and_then(|v| v.as_str()),
        Some("sentence")
    );
    assert_eq!(
        props.get("origin_ref").and_then(|v| v.as_str()),
        Some("sent:1")
    );
    assert_eq!(props.get("valid_from").and_then(|v| v.as_i64()), Some(5));
    assert_eq!(props.get("valid_to").and_then(|v| v.as_i64()), Some(10));
}
