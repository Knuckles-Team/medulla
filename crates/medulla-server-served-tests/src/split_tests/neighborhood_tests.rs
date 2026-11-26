//! MEDULLA-DURABLE-KERNEL-R876: one bounded neighborhood read serves interactive graph
//! exploration.
//!
//! Every read goes through the real dispatch chokepoint (`dispatch_verified_request`) as a
//! verified tenant context, so the read scope separates tenants the way a served read does.
//! The graph is seeded on the registry's core: the tests are about the read path.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use medulla_protocol_types::protocol::{GraphType, Method, Request, Response, ResultPayload};
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_server_api::state::ServerState;
use medulla_types::types::NeighborhoodResult;
use tokio::sync::RwLock;

const SECRET: &str = "neighborhood-secret"; // sanitizer:ignore
const TENANT: &str = "tenant-shared";
const OTHER_TENANT: &str = "tenant-other";
const ADMIN: &str = "neighborhood-admin";
const COMMONS: &str = "__commons__";

fn next_id() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

struct Served {
    state: Arc<RwLock<ServerState>>,
}

impl Served {
    async fn new() -> Self {
        let state = medulla_server::seams_install::new_state_for_test(
            SECRET,
            ServerState::test_isolation(ADMIN),
        );
        let served = Self {
            state: Arc::new(RwLock::new(state)),
        };
        {
            let mut guard = served.state.write().await;
            if guard.registry.get(COMMONS).is_none() {
                guard
                    .registry
                    .create_graph(COMMONS, GraphType::Team, None)
                    .unwrap();
            }
        }
        served
    }

    async fn core(&self) -> Arc<medulla_graph_core::graph::GraphCore> {
        let guard = self.state.read().await;
        guard.registry.get(COMMONS).unwrap().core.clone()
    }

    /// One node owned by `tenant`.
    async fn node(&self, id: &str, tenant: &str) {
        let core = self.core().await;
        core.put_node(
            id.to_string(),
            rmp_serde::to_vec_named(&serde_json::json!({
                "type": "Doc", "node_type": "Doc", "text": id,
                "owner_tenant": tenant, "_tenant": tenant,
            }))
            .unwrap(),
        );
    }

    /// A `LINKS` edge from `source` to `target`.
    async fn link(&self, source: &str, target: &str) {
        let core = self.core().await;
        core.add_edge(
            source.to_string(),
            target.to_string(),
            rmp_serde::to_vec_named(&serde_json::json!({"relationship": "LINKS"})).unwrap(),
        )
        .unwrap();
        core.mark_dirty();
    }

    async fn read_as(&self, tenant: &str, params: serde_json::Value) -> Response {
        let method: Method = serde_json::from_value(
            serde_json::json!({"method": "GetNeighborhood", "params": params}),
        )
        .expect("a GetNeighborhood wire method");
        let request = Request {
            id: next_id(),
            graph: COMMONS.into(),
            auth_token: String::new(),
            agent_id: Some(ADMIN.into()),
            method,
            trace_id: None,
        };
        let verified =
            VerifiedRequestContext::verified_for_test_with_scopes(ADMIN, tenant, &["kg:read"]);
        Box::pin(
            medulla_server_router::dispatch::request_boundary::dispatch_verified_request(
                &self.state,
                request,
                verified,
            ),
        )
        .await
    }
}

fn answer(response: Response) -> NeighborhoodResult {
    assert!(
        response.error.is_none(),
        "neighborhood read failed: {:?}",
        response.refusal_text()
    );
    match response.result {
        Some(ResultPayload::Json(value)) => serde_json::from_value(value).expect("a result"),
        Some(ResultPayload::Raw(bytes)) => rmp_serde::from_slice(&bytes).expect("a result"),
        other => panic!("expected a neighborhood result, got {other:?}"),
    }
}

fn refusal(response: Response) -> String {
    response.refusal_text().expect("a refusal")
}

fn params(node_id: &str, depth: u32, per_node: u32, total: u32) -> serde_json::Value {
    serde_json::json!({
        "node_id": node_id, "depth": depth,
        "per_node_limit": per_node, "total_limit": total,
    })
}

fn ids(result: &NeighborhoodResult) -> Vec<String> {
    result.nodes.iter().map(|node| node.id.clone()).collect()
}

// spec: MEDULLA-DURABLE-KERNEL-R876.3
#[tokio::test(flavor = "multi_thread")]
async fn read_is_classified_and_tenant_filtered() {
    // The method is a classified read: a read-only caller may call it.
    let wire: Method = serde_json::from_value(serde_json::json!({
        "method": "GetNeighborhood",
        "params": params("hub", 1, 10, 10),
    }))
    .unwrap();
    assert!(!medulla_server_auth::access::requires_write(&wire));

    let served = Served::new().await;
    served.node("hub", TENANT).await;
    for index in 0..3 {
        let id = format!("own-{index}");
        served.node(&id, TENANT).await;
        served.link("hub", &id).await;
    }
    for index in 0..2 {
        let id = format!("foreign-{index}");
        served.node(&id, OTHER_TENANT).await;
        served.link("hub", &id).await;
    }

    let own = answer(served.read_as(TENANT, params("hub", 1, 10, 10)).await);
    let mut seen = ids(&own);
    seen.sort();
    assert_eq!(seen, ["hub", "own-0", "own-1", "own-2"]);
    assert!(
        own.hidden.iter().all(|h| h.count == 0 || h.node_id != "hub"),
        "a hidden count never includes a neighbor the caller may not read: {:?}",
        own.hidden
    );
    assert!(own.edges.iter().all(|edge| !edge.target.starts_with("foreign")));

    // The other tenant cannot see the hub: it is not found, not forbidden.
    let text = refusal(served.read_as(OTHER_TENANT, params("hub", 1, 10, 10)).await);
    assert!(text.starts_with("NODE_NOT_FOUND"), "{text}");
}

// spec: MEDULLA-DURABLE-KERNEL-R876.4
#[tokio::test(flavor = "multi_thread")]
async fn refusals_and_visibility() {
    let served = Served::new().await;
    served.node("hub", TENANT).await;
    served.node("leaf", TENANT).await;
    served.link("hub", "leaf").await;

    for (label, request, code) in [
        ("depth 0", params("hub", 0, 10, 10), "NEIGHBORHOOD_DEPTH_EXCEEDED"),
        ("depth 4", params("hub", 4, 10, 10), "NEIGHBORHOOD_DEPTH_EXCEEDED"),
        ("no per-node limit", params("hub", 1, 0, 10), "NEIGHBORHOOD_LIMIT_REQUIRED"),
        ("no total limit", params("hub", 1, 10, 0), "NEIGHBORHOOD_LIMIT_REQUIRED"),
        ("total above 5000", params("hub", 1, 10, 5_001), "NEIGHBORHOOD_LIMIT_EXCEEDED"),
        ("unknown node", params("nobody", 1, 10, 10), "NODE_NOT_FOUND"),
    ] {
        let text = refusal(served.read_as(TENANT, request).await);
        assert!(text.starts_with(code), "{label}: {text}");
    }

    let mut too_many = params("hub", 1, 10, 10);
    too_many["exclude"] = serde_json::json!((0..5_001).map(|i| i.to_string()).collect::<Vec<_>>());
    let text = refusal(served.read_as(TENANT, too_many).await);
    assert!(text.starts_with("NEIGHBORHOOD_EXCLUDE_TOO_LARGE"), "{text}");

    // A cursor is valid only at the graph revision it was issued at.
    let core = served.core().await;
    for index in 0..4 {
        let id = format!("page-{index}");
        served.node(&id, TENANT).await;
        served.link("hub", &id).await;
    }
    let first = answer(served.read_as(TENANT, params("hub", 1, 2, 10)).await);
    let cursor = first.next_cursor.clone().expect("the hub dropped neighbors");
    let mut resumed = params("hub", 1, 2, 10);
    resumed["cursor"] = serde_json::json!(cursor);
    let page = answer(served.read_as(TENANT, resumed.clone()).await);
    assert_eq!(page.graph_revision, first.graph_revision);
    core.mark_dirty();
    let text = refusal(served.read_as(TENANT, resumed).await);
    assert!(text.starts_with("NEIGHBORHOOD_CURSOR_STALE"), "{text}");
}

// spec: MEDULLA-DURABLE-KERNEL-R876.5
#[tokio::test(flavor = "multi_thread")]
async fn audit_line_carries_counts_only() {
    let served = Served::new().await;
    served.node("hub", TENANT).await;
    served.node("leaf", TENANT).await;
    served.link("hub", "leaf").await;
    let result = answer(served.read_as(TENANT, params("hub", 1, 10, 10)).await);
    assert!(result.graph_revision > 0, "the result carries the graph revision");

    let line = medulla_graph_core::neighborhood::audit_line(
        "hub",
        1,
        result.nodes.len(),
        result.edges.len(),
        result.truncated,
    );
    assert_eq!(
        line,
        "GET_NEIGHBORHOOD|hub|depth=1|nodes=2|edges=1|truncated=false"
    );
    // No property value ("text": "leaf", "Doc") reaches the line.
    assert!(!line.contains("Doc") && !line.contains("leaf"));
}

// spec: MEDULLA-DURABLE-KERNEL-R876.6
#[tokio::test(flavor = "multi_thread")]
async fn hub_pages_to_completion() {
    let served = Served::new().await;
    served.node("hub", TENANT).await;
    for index in 0..2_000 {
        let id = format!("leaf-{index:04}");
        served.node(&id, TENANT).await;
        served.link("hub", &id).await;
    }

    let first = answer(served.read_as(TENANT, params("hub", 1, 50, 500)).await);
    assert_eq!(first.nodes.len(), 51, "the hub and its first 50 neighbors");
    assert_eq!(
        first
            .hidden
            .iter()
            .find(|h| h.node_id == "hub")
            .map(|h| h.count),
        Some(1_950)
    );
    assert!(first.truncated);

    let mut collected: std::collections::BTreeSet<String> = ids(&first).into_iter().collect();
    let mut cursor = first.next_cursor;
    let mut pages = 1;
    while let Some(next) = cursor {
        let mut request = params("hub", 1, 50, 500);
        request["cursor"] = serde_json::json!(next);
        let page = answer(served.read_as(TENANT, request).await);
        collected.extend(ids(&page));
        cursor = page.next_cursor;
        pages += 1;
        assert!(pages <= 40, "paging does not end");
    }
    assert_eq!(pages, 40, "2,000 neighbors in pages of 50");
    assert_eq!(collected.len(), 2_001, "the hub and every neighbor, once");
}

// spec: MEDULLA-DURABLE-KERNEL-R876
#[tokio::test(flavor = "multi_thread")]
async fn neighborhood_journey() {
    let served = Served::new().await;
    for id in ["root", "a", "b", "c"] {
        served.node(id, TENANT).await;
    }
    served.link("root", "a").await;
    served.link("root", "b").await;
    served.link("a", "c").await;

    let result = answer(served.read_as(TENANT, params("root", 2, 10, 10)).await);
    let mut seen = ids(&result);
    seen.sort();
    assert_eq!(seen, ["a", "b", "c", "root"]);
    assert_eq!(result.edges.len(), 3);
    assert!(!result.truncated && result.next_cursor.is_none());

    // The viewer already shows `a`: it is excluded, not counted and not re-sent.
    let mut with_exclude = params("root", 1, 10, 10);
    with_exclude["exclude"] = serde_json::json!(["a"]);
    let result = answer(served.read_as(TENANT, with_exclude).await);
    let mut seen = ids(&result);
    seen.sort();
    assert_eq!(seen, ["b", "root"]);
}
