//! Served proofs of the graph change feed (MEDULLA-DURABLE-KERNEL-R043.100.2,
//! R043.100.3): `OpenGraphSnapshot` and `ReadGraphChanges` through the real
//! request boundary, over a durable engine.
#![cfg(all(feature = "server", feature = "security", feature = "streaming"))]

mod common;
#[path = "common/test_support.rs"]
mod test_support;

use medulla_protocol_types::protocol::{Method, Response, ResultPayload};
use medulla_types::graph_change_feed::{GraphChange, GraphChangePage, GraphSnapshot};

const SECRET: &str = "r043-100-graph-change-feed-secret";
const GRAPH: &str = "__commons__";

fn state() -> test_support::SharedState {
    test_support::durable_state(SECRET, common::current_isolation())
}

async fn call(state: &test_support::SharedState, id: u64, method: Method) -> Response {
    test_support::dispatch(state, test_support::request(SECRET, id, GRAPH, method)).await
}

async fn call_scoped(
    state: &test_support::SharedState,
    id: u64,
    scopes: &[&str],
    method: Method,
) -> Response {
    test_support::dispatch(
        state,
        common::signed_request_with_scopes(SECRET, id, GRAPH, scopes, method),
    )
    .await
}

async fn add_node(state: &test_support::SharedState, id: u64, node: &str) {
    let response = call(
        state,
        id,
        Method::AddNode {
            node_id: node.to_string(),
            properties_msgpack: rmp_serde::to_vec_named(&serde_json::json!({"type": "Doc"}))
                .unwrap(),
        },
    )
    .await;
    assert!(response.error.is_none(), "AddNode {node}: {response:?}");
}

fn snapshot_of(response: &Response) -> GraphSnapshot {
    assert!(response.error.is_none(), "snapshot: {response:?}");
    match &response.result {
        Some(ResultPayload::Json(value)) => {
            serde_json::from_value(value.clone()).expect("snapshot")
        }
        other => panic!("expected a snapshot, got {other:?}"),
    }
}

fn page_of(response: &Response) -> GraphChangePage {
    assert!(response.error.is_none(), "page: {response:?}");
    match &response.result {
        Some(ResultPayload::Json(value)) => serde_json::from_value(value.clone()).expect("page"),
        other => panic!("expected a page, got {other:?}"),
    }
}

fn upserted(page: &GraphChangePage) -> Vec<String> {
    page.events
        .iter()
        .flat_map(|event| match &event.change {
            GraphChange::Upsert { node_ids, .. } => node_ids.clone(),
            GraphChange::Tombstone { .. } => Vec::new(),
        })
        .collect()
}

// spec: MEDULLA-DURABLE-KERNEL-R043.100.2
#[tokio::test]
async fn snapshot_cursor_leaves_no_gap_under_concurrent_commits() {
    let state = state();
    add_node(&state, 1, "before-1").await;
    add_node(&state, 2, "before-2").await;

    let first = snapshot_of(&call(&state, 4, Method::OpenGraphSnapshot).await);
    assert_eq!(first.committed_revision, 2);

    // Four commits race the snapshot.
    let (snapshot, (), (), (), ()) = tokio::join!(
        call(&state, 3, Method::OpenGraphSnapshot),
        add_node(&state, 10, "race-0"),
        add_node(&state, 11, "race-1"),
        add_node(&state, 12, "race-2"),
        add_node(&state, 13, "race-3"),
    );
    let snapshot = snapshot_of(&snapshot);
    assert!(
        (2..=6).contains(&snapshot.committed_revision),
        "the cursor sits between the two commits before it and all six: {snapshot:?}"
    );
    assert_eq!(snapshot.snapshot_digest.len(), 64, "a SHA-256 digest");

    // Paging from the cursor delivers every commit the snapshot did not
    // cover exactly once and in order: position plus delivered equals all six.
    let mut cursor = snapshot.high_water_cursor.clone();
    let mut seen = Vec::new();
    for round in 0..8_u64 {
        let page = page_of(
            &call(
                &state,
                100 + round,
                Method::ReadGraphChanges {
                    cursor: cursor.clone(),
                    limit: 3,
                },
            )
            .await,
        );
        seen.extend(upserted(&page));
        cursor = page.next_cursor;
    }
    assert_eq!(
        seen.len() as u64 + snapshot.committed_revision,
        6,
        "no commit falls between the digest and the cursor: {seen:?}"
    );
    let mut unique = seen.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), seen.len(), "no event is delivered twice");

    // A second snapshot after the commits names the new head and a new digest.
    let later = snapshot_of(&call(&state, 200, Method::OpenGraphSnapshot).await);
    assert_eq!(later.committed_revision, 6);
    assert_ne!(later.snapshot_digest, first.snapshot_digest);
    assert_eq!(later.incarnation, first.incarnation);
}

// spec: MEDULLA-DURABLE-KERNEL-R043.100.3
#[tokio::test]
async fn expired_stale_and_revoked_cursors_return_typed_results_without_leaks() {
    let state = state();
    add_node(&state, 1, "secret-node-name").await;
    let snapshot = snapshot_of(&call(&state, 2, Method::OpenGraphSnapshot).await);

    // A live cursor reads the commit that follows it.
    add_node(&state, 3, "later-node").await;
    let page = page_of(
        &call(
            &state,
            4,
            Method::ReadGraphChanges {
                cursor: snapshot.high_water_cursor.clone(),
                limit: 10,
            },
        )
        .await,
    );
    assert_eq!(upserted(&page), vec!["later-node".to_string()]);
    assert!(page.events[0].commit_to_visible_ms < 1_000, "{page:?}");

    // A page size outside 1..=256 is refused, never clamped.
    for limit in [0_u32, 257] {
        let refused = call(
            &state,
            5,
            Method::ReadGraphChanges {
                cursor: snapshot.high_water_cursor.clone(),
                limit,
            },
        )
        .await;
        assert_eq!(refused.error.as_deref(), Some("INVALID_ARGUMENT"));
    }

    // A permission change refuses the old cursor and reveals nothing.
    let revoked = call_scoped(
        &state,
        6,
        &["graph:read"],
        Method::ReadGraphChanges {
            cursor: snapshot.high_water_cursor.clone(),
            limit: 10,
        },
    )
    .await;
    assert_eq!(revoked.error.as_deref(), Some("AUTHORIZATION_CHANGED"));
    assert!(revoked.result.is_none());
    let detail = revoked.error_detail.clone().unwrap_or_default();
    assert!(!detail.contains("secret-node-name"), "{detail}");
    assert!(!detail.contains("later-node"), "{detail}");

    // Recreating the graph's feed makes the old incarnation stale and names a
    // safe cursor that reads again.
    assert!(call(&state, 7, Method::ClearGraph).await.error.is_none());
    let stale = call(
        &state,
        8,
        Method::ReadGraphChanges {
            cursor: snapshot.high_water_cursor.clone(),
            limit: 10,
        },
    )
    .await;
    assert_eq!(stale.error.as_deref(), Some("GRAPH_INCARNATION_STALE"));
    let detail = stale.error_detail.clone().unwrap_or_default();
    let safe = detail
        .split("safe_cursor=")
        .nth(1)
        .expect("a safe cursor is named")
        .trim()
        .to_string();
    add_node(&state, 9, "after-clear").await;
    let resumed = page_of(
        &call(
            &state,
            10,
            Method::ReadGraphChanges {
                cursor: safe,
                limit: 10,
            },
        )
        .await,
    );
    assert_eq!(upserted(&resumed), vec!["after-clear".to_string()]);

    // A cursor ahead of the feed's head names events this feed never produced:
    // it is expired, and the refusal carries a cursor that reads again.
    let ahead = snapshot_of(&call(&state, 11, Method::OpenGraphSnapshot).await);
    let tenant = medulla_server_api::carrier::tenant_scope_of(common::TEST_TENANT);
    let ahead_cursor = {
        let bound = medulla_types::graph_change_feed::GraphChangeCursor::decode(
            &tenant,
            &ahead.high_water_cursor,
        )
        .expect("own cursor decodes");
        medulla_types::graph_change_feed::GraphChangeCursor {
            last_event: bound.last_event + 50,
            ..bound
        }
        .encode(&tenant)
    };
    let expired = call(
        &state,
        12,
        Method::ReadGraphChanges {
            cursor: ahead_cursor,
            limit: 10,
        },
    )
    .await;
    assert_eq!(expired.error.as_deref(), Some("CURSOR_EXPIRED"));
    assert!(expired
        .error_detail
        .clone()
        .unwrap_or_default()
        .contains("safe_cursor="));

    // A cursor of another tenant is not a cursor.
    let foreign = medulla_types::graph_change_feed::GraphChangeCursor {
        graph: GRAPH.to_string(),
        incarnation: ahead.incarnation,
        authorization_revision: ahead.visibility_revision,
        last_event: 0,
    }
    .encode("another-tenant");
    let refused = call(
        &state,
        13,
        Method::ReadGraphChanges {
            cursor: foreign,
            limit: 10,
        },
    )
    .await;
    assert_eq!(refused.error.as_deref(), Some("INVALID_ARGUMENT"));
}
