//! Served write-conflict resolution on `ApplyChangeEnvelope`
//! (MEDULLA-DURABLE-KERNEL-R084.3 .. R084.6).
//!
//! Every envelope here is an `ApplyChangeEnvelopeDraft` the request boundary
//! compiles under the signed request's verified tenant and principal context,
//! so the stale expectation reaches `validate_change_envelope_version` and
//! `resolve_envelope_conflict` through the real served dispatch chain.
#![cfg(all(feature = "server", feature = "security", feature = "redb"))]

mod common;
#[path = "common/test_support.rs"]
mod test_support;

use std::collections::BTreeMap;

use medulla_protocol_types::protocol::{GraphType, Method, Response, ResultPayload};
use medulla_types::write_conflict::MergeStrategy;
use serde_json::{json, Value};

const SECRET: &str = "write-conflict-two-writers-secret";
const TENANT: &str = "integration-test-tenant";

fn new_state() -> test_support::SharedState {
    common::configure_authority();
    test_support::durable_state(SECRET, common::current_isolation())
}

/// Every request takes a fresh id so no two signed requests share a nonce.
async fn call(
    state: &test_support::SharedState,
    _id: u64,
    graph: &str,
    method: Method,
) -> Response {
    static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let id = NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    test_support::dispatch(state, test_support::request(SECRET, id, graph, method)).await
}

async fn graph(state: &test_support::SharedState, name: &str, strategy: MergeStrategy) {
    state.write().await.write_conflict_strategy = strategy;
    let created = call(
        state,
        1,
        name,
        Method::CreateGraph {
            graph_name: name.to_owned(),
            graph_type: GraphType::Global,
        },
    )
    .await;
    assert!(created.error.is_none(), "{created:?}");
}

/// One `AddNode` envelope for `node` carrying `properties`, stamped with
/// `expected` as its graph version expectation. `tag` keeps envelope ids unique.
fn envelope(tag: &str, node: &str, properties: Value, expected: u64) -> Method {
    let mut draft =
        medulla_server_graph::dispatch::change_envelope_draft::tests::draft_for(TENANT, properties);
    draft.envelope_id = format!("envelope:{tag}");
    draft.mutation.batch_id = format!("batch:{tag}");
    draft.mutation.expected_graph_version = Some(expected);
    if let medulla_protocol_types::protocol::Method::AddNode { node_id, .. } =
        &mut draft.mutation.operations[0].method
    {
        *node_id = node.to_owned();
    }
    draft.content_version.object_id = node.to_owned();
    draft.policies[0].object_id = node.to_owned();
    draft.policies[0].policy_id = format!("policy:{tag}");
    Method::ApplyChangeEnvelopeDraft {
        draft: Box::new(draft),
    }
}

async fn version(state: &test_support::SharedState, name: &str) -> u64 {
    state
        .read()
        .await
        .registry
        .get(name)
        .expect("graph exists")
        .core
        .version()
}

async fn properties(
    state: &test_support::SharedState,
    name: &str,
    node: &str,
) -> Option<BTreeMap<String, Value>> {
    let guard = state.read().await;
    let blob = guard
        .registry
        .get(name)
        .expect("graph exists")
        .core
        .get_node_properties(node)?;
    Some(rmp_serde::from_slice(&blob).expect("node properties decode"))
}

fn result_json(response: &Response) -> Value {
    match &response.result {
        Some(ResultPayload::Json(value)) => value.clone(),
        other => panic!("expected a JSON result, got {other:?}"),
    }
}

/// The `medulla_write_conflicts_total` sample (0 without the `metrics`
/// feature, where `moved` skips the comparison).
fn conflicts_total(strategy: &str, winner: &str) -> u64 {
    if !cfg!(feature = "metrics") {
        return 0;
    }
    let prefix =
        format!("medulla_write_conflicts_total{{strategy=\"{strategy}\",winner=\"{winner}\"}} ");
    medulla_kernel::metrics::render()
        .lines()
        .find_map(|line| line.strip_prefix(prefix.as_str()))
        .and_then(|value| value.trim().parse().ok())
        .unwrap_or(0)
}

/// Seed `node` with `{"a": 1}` and return the version before the seed (the
/// stale expectation) and after it (the authoritative version).
async fn seeded(state: &test_support::SharedState, name: &str, node: &str) -> (u64, u64) {
    let before = version(state, name).await;
    let seed = call(
        state,
        2,
        name,
        envelope(
            &format!("{name}:seed"),
            node,
            json!({"type": "Document", "a": 1}),
            before,
        ),
    )
    .await;
    assert!(seed.error.is_none(), "{seed:?}");
    (before, version(state, name).await)
}

/// The counter moved past `before` (always true without `metrics`).
fn moved(strategy: &str, winner: &str, before: u64) -> bool {
    !cfg!(feature = "metrics") || conflicts_total(strategy, winner) > before
}

// spec: MEDULLA-DURABLE-KERNEL-R084.3
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stale_envelope_reaches_the_resolver() {
    for (strategy, label, winner) in [
        (MergeStrategy::Refuse, "refuse", "existing"),
        (MergeStrategy::PreferNewer, "prefer_newer", "incoming"),
        (MergeStrategy::FieldUnion, "field_union", "merged"),
    ] {
        let state = new_state();
        let name = format!("wc-resolver-{label}");
        graph(&state, &name, strategy).await;
        let (stale, actual) = seeded(&state, &name, "doc:n").await;
        let counted = conflicts_total(label, winner);
        let response = call(
            &state,
            3,
            &name,
            envelope(
                &format!("{name}:stale"),
                "doc:n",
                json!({"type": "Document", "b": 2}),
                stale,
            ),
        )
        .await;
        assert!(moved(label, winner, counted), "{label}: the resolver ran");
        if strategy == MergeStrategy::Refuse {
            assert_eq!(
                response.error.as_deref(),
                Some("WRITE_CONFLICT"),
                "{response:?}"
            );
            assert_eq!(version(&state, &name).await, actual);
        } else {
            assert!(response.error.is_none(), "{label}: {response:?}");
            assert_eq!(version(&state, &name).await, actual + 1);
            let stored = properties(&state, &name, "doc:n").await.expect("node");
            assert_eq!(stored.get("b"), Some(&json!(2)));
            assert_eq!(
                stored.contains_key("a"),
                strategy == MergeStrategy::FieldUnion,
                "{label}: {stored:?}"
            );
        }
    }
}

// spec: MEDULLA-DURABLE-KERNEL-R084.4
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conflict_codes_leave_state_unchanged() {
    // Refuse: WRITE_CONFLICT with the three values, nothing changes.
    let state = new_state();
    graph(&state, "wc-codes-refuse", MergeStrategy::Refuse).await;
    let (stale, actual) = seeded(&state, "wc-codes-refuse", "doc:n").await;
    let before = properties(&state, "wc-codes-refuse", "doc:n").await;
    let refused = call(
        &state,
        3,
        "wc-codes-refuse",
        envelope("wc-codes-refuse:stale", "doc:n", json!({"a": 9}), stale),
    )
    .await;
    assert_eq!(
        refused.error.as_deref(),
        Some("WRITE_CONFLICT"),
        "{refused:?}"
    );
    let detail = refused.error_detail.clone().unwrap_or_default();
    assert!(
        detail.contains(&format!(
            "expected {stale}, actual {actual}, scope wc-codes-refuse"
        )),
        "{detail}"
    );
    assert_eq!(version(&state, "wc-codes-refuse").await, actual);
    assert_eq!(properties(&state, "wc-codes-refuse", "doc:n").await, before);

    // FieldUnion: a field collision and an AddNode on an absent node are
    // both unmergeable, and neither changes state or version.
    let state = new_state();
    graph(&state, "wc-codes-union", MergeStrategy::FieldUnion).await;
    let (stale, actual) = seeded(&state, "wc-codes-union", "doc:n").await;
    let before = properties(&state, "wc-codes-union", "doc:n").await;
    for (tag, node, props) in [
        ("collide", "doc:n", json!({"type": "Document", "a": 9})),
        ("absent", "doc:absent", json!({"type": "Document", "z": 1})),
    ] {
        let response = call(
            &state,
            4,
            "wc-codes-union",
            envelope(&format!("wc-codes-union:{tag}"), node, props, stale),
        )
        .await;
        assert_eq!(
            response.error.as_deref(),
            Some("WRITE_CONFLICT_UNMERGEABLE"),
            "{tag}: {response:?}"
        );
        let detail = response.error_detail.clone().unwrap_or_default();
        assert!(
            detail.contains(&format!(
                "expected {stale}, actual {actual}, scope wc-codes-union"
            )),
            "{tag}: {detail}"
        );
        assert_eq!(version(&state, "wc-codes-union").await, actual);
    }
    assert_eq!(properties(&state, "wc-codes-union", "doc:n").await, before);
    assert_eq!(
        properties(&state, "wc-codes-union", "doc:absent").await,
        None
    );
}

// spec: MEDULLA-DURABLE-KERNEL-R084.5
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conflict_evidence_names_scope_strategy_winner() {
    let state = new_state();
    graph(&state, "wc-evidence-refuse", MergeStrategy::Refuse).await;
    let (stale, _) = seeded(&state, "wc-evidence-refuse", "doc:n").await;
    let refusals = conflicts_total("refuse", "existing");
    let refused = call(
        &state,
        3,
        "wc-evidence-refuse",
        envelope("wc-evidence-refuse:stale", "doc:n", json!({"b": 2}), stale),
    )
    .await;
    let detail = refused.error_detail.clone().unwrap_or_default();
    for needle in [
        "\"conflict_scope\":\"wc-evidence-refuse\"",
        "\"strategy\":\"refuse\"",
        "\"winner\":\"existing\"",
    ] {
        assert!(detail.contains(needle), "{needle} in {detail}");
    }
    assert!(moved("refuse", "existing", refusals));

    let state = new_state();
    graph(&state, "wc-evidence-union", MergeStrategy::FieldUnion).await;
    let (stale, _) = seeded(&state, "wc-evidence-union", "doc:n").await;
    let merges = conflicts_total("field_union", "merged");
    let merged = call(
        &state,
        3,
        "wc-evidence-union",
        envelope(
            "wc-evidence-union:stale",
            "doc:n",
            json!({"type": "Document", "b": 2}),
            stale,
        ),
    )
    .await;
    assert!(merged.error.is_none(), "{merged:?}");
    assert_eq!(
        result_json(&merged)["write_conflict"],
        json!({
            "conflict_scope": "wc-evidence-union",
            "strategy": "field_union",
            "winner": "merged",
        })
    );
    assert!(moved("field_union", "merged", merges));
}

// spec: MEDULLA-DURABLE-KERNEL-R084.6
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_writers_one_commit_one_typed_conflict() {
    let state = new_state();
    graph(&state, "wc-two-writers", MergeStrategy::Refuse).await;
    for round in 0..50u64 {
        let v = version(&state, "wc-two-writers").await;
        let (left, right) = tokio::join!(
            call(
                &state,
                10 + round * 2,
                "wc-two-writers",
                envelope(
                    &format!("two:{round}:left"),
                    &format!("doc:{round}:left"),
                    json!({"type": "Document", "side": "left"}),
                    v,
                ),
            ),
            call(
                &state,
                11 + round * 2,
                "wc-two-writers",
                envelope(
                    &format!("two:{round}:right"),
                    &format!("doc:{round}:right"),
                    json!({"type": "Document", "side": "right"}),
                    v,
                ),
            ),
        );
        let (winner, loser, loser_node) = match (left.error.is_none(), right.error.is_none()) {
            (true, false) => (left, right, format!("doc:{round}:right")),
            (false, true) => (right, left, format!("doc:{round}:left")),
            other => panic!("round {round}: exactly one commit, got {other:?}: {left:?} {right:?}"),
        };
        assert!(winner.error.is_none());
        assert_eq!(
            loser.error.as_deref(),
            Some("WRITE_CONFLICT"),
            "round {round}"
        );
        let detail = loser.error_detail.clone().unwrap_or_default();
        assert!(
            detail.contains(&format!("expected {v}, actual {}", v + 1)),
            "round {round}: {detail}"
        );
        assert_eq!(
            version(&state, "wc-two-writers").await,
            v + 1,
            "round {round}"
        );
        assert_eq!(
            properties(&state, "wc-two-writers", &loser_node).await,
            None,
            "round {round}: no silent write"
        );
    }

    // FieldUnion: two writers on disjoint properties of one node both commit.
    let state = new_state();
    graph(&state, "wc-two-union", MergeStrategy::FieldUnion).await;
    let (_, v) = seeded(&state, "wc-two-union", "doc:n").await;
    let (left, right) = tokio::join!(
        call(
            &state,
            3,
            "wc-two-union",
            envelope(
                "two-union:left",
                "doc:n",
                json!({"type": "Document", "a": 1, "l": 1}),
                v
            ),
        ),
        call(
            &state,
            4,
            "wc-two-union",
            envelope(
                "two-union:right",
                "doc:n",
                json!({"type": "Document", "a": 1, "r": 1}),
                v
            ),
        ),
    );
    assert!(left.error.is_none(), "{left:?}");
    assert!(right.error.is_none(), "{right:?}");
    let stored = properties(&state, "wc-two-union", "doc:n")
        .await
        .expect("node");
    assert_eq!(stored.get("l"), Some(&json!(1)));
    assert_eq!(stored.get("r"), Some(&json!(1)));
}
