//! MEDULLA-FEDERATED-QUERY-R743: `CommunityDetection`, `PageRank` and `GetShortestPath` accept an
//! optional `node_ids` scope, limited to the caller's visible set, and answer typed views.
//!
//! Every read goes through the real dispatch chokepoint (`dispatch_verified_request`) as a
//! verified tenant context, so the read projection hides the other tenant's rows the way a served
//! read does. The graph is seeded on the registry's core: the tests are about the read path.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use medulla_protocol_types::protocol::{GraphType, Method, Request, Response, ResultPayload};
use medulla_result_types::compute_result::algorithms::{
    AlgorithmScopeView, CommunityDetectionView, PageRankView, ShortestPathView,
};
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_server_api::state::ServerState;
use tokio::sync::RwLock;

const SECRET: &str = "algorithm-scope-secret"; // sanitizer:ignore
const TENANT: &str = "tenant-shared";
const OTHER_TENANT: &str = "tenant-other";
const ADMIN: &str = "algorithm-scope-admin";
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

    /// The chain `a -> b -> c -> d` of `TENANT` nodes plus `x`, a node of `OTHER_TENANT` that the
    /// chain also links through (`b -> x -> c`).
    async fn seeded() -> Self {
        let served = Self::new().await;
        for id in ["a", "b", "c", "d"] {
            served.node(id, TENANT).await;
        }
        served.node("x", OTHER_TENANT).await;
        for (source, target) in [("a", "b"), ("b", "c"), ("c", "d"), ("b", "x"), ("x", "c")] {
            served.link(source, target).await;
        }
        served
    }

    async fn read_as(&self, tenant: &str, method: serde_json::Value) -> Response {
        self.read_with_scopes(tenant, &["kg:read", "compute:graph-algo"], method)
            .await
    }

    async fn read_with_scopes(
        &self,
        tenant: &str,
        scopes: &[&str],
        method: serde_json::Value,
    ) -> Response {
        let method: Method = serde_json::from_value(method).expect("a wire method");
        let request = Request {
            id: next_id(),
            graph: COMMONS.into(),
            auth_token: String::new(),
            agent_id: Some(ADMIN.into()),
            method,
            trace_id: None,
        };
        let verified = VerifiedRequestContext::verified_for_test_with_scopes(
            ADMIN,
            tenant,
            scopes,
        );
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

fn json_of(response: Response) -> serde_json::Value {
    assert!(
        response.error.is_none(),
        "algorithm read failed: {:?}",
        response.refusal_text()
    );
    match response.result {
        Some(ResultPayload::Json(value)) => value,
        other => panic!("expected a Json view, got {other:?}"),
    }
}

fn page_rank(node_ids: Option<&[&str]>) -> serde_json::Value {
    let mut params = serde_json::json!({"damping": 0.85, "iterations": 20});
    if let Some(ids) = node_ids {
        params["node_ids"] = serde_json::json!(ids);
    }
    serde_json::json!({"method": "PageRank", "params": params})
}

fn shortest_path(node_ids: Option<&[&str]>) -> serde_json::Value {
    let mut params = serde_json::json!({"source_id": "a", "target_id": "d"});
    if let Some(ids) = node_ids {
        params["node_ids"] = serde_json::json!(ids);
    }
    serde_json::json!({"method": "GetShortestPath", "params": params})
}

fn communities(node_ids: Option<&[&str]>) -> serde_json::Value {
    let mut params = serde_json::json!({"resolution": 1.0});
    if let Some(ids) = node_ids {
        params["node_ids"] = serde_json::json!(ids);
    }
    serde_json::json!({"method": "CommunityDetection", "params": params})
}

// spec: MEDULLA-FEDERATED-QUERY-R743.1
#[tokio::test(flavor = "multi_thread")]
async fn an_unscoped_read_answers_the_visible_graph_with_no_scope_field() {
    let served = Served::seeded().await;
    let rank: PageRankView =
        serde_json::from_value(json_of(served.read_as(TENANT, page_rank(None)).await)).unwrap();
    let mut ranked: Vec<_> = rank.scores.iter().map(|s| s.node_id.as_str()).collect();
    ranked.sort_unstable();
    assert_eq!(
        ranked,
        ["a", "b", "c", "d"],
        "the other tenant's node is not ranked"
    );
    assert_eq!(rank.scope, None);

    let path: ShortestPathView =
        serde_json::from_value(json_of(served.read_as(TENANT, shortest_path(None)).await)).unwrap();
    assert_eq!(path.scope, None);
    assert_eq!(
        path.path,
        Some(vec!["a".into(), "b".into(), "c".into(), "d".into()])
    );

    let wire = json_of(served.read_as(TENANT, communities(None)).await);
    assert!(
        wire.get("scope").is_none(),
        "no scope key without node_ids: {wire}"
    );
    let partition: CommunityDetectionView = serde_json::from_value(wire).unwrap();
    assert_eq!(partition.communities.iter().map(Vec::len).sum::<usize>(), 4);
}

// spec: MEDULLA-FEDERATED-QUERY-R743.3
#[tokio::test(flavor = "multi_thread")]
async fn a_scope_runs_on_the_induced_subgraph_and_reports_what_it_selected() {
    let served = Served::seeded().await;
    let rank: PageRankView = serde_json::from_value(json_of(
        served
            .read_as(TENANT, page_rank(Some(&["a", "b", "c"])))
            .await,
    ))
    .unwrap();
    let mut ranked: Vec<_> = rank.scores.iter().map(|s| s.node_id.as_str()).collect();
    ranked.sort_unstable();
    assert_eq!(ranked, ["a", "b", "c"]);
    assert_eq!(
        rank.scope,
        Some(AlgorithmScopeView {
            requested: 3,
            in_scope: 3
        })
    );

    // `c` is outside the scope, so the induced subgraph holds no path from `a` to `d`.
    let path: ShortestPathView = serde_json::from_value(json_of(
        served
            .read_as(TENANT, shortest_path(Some(&["a", "b", "d"])))
            .await,
    ))
    .unwrap();
    assert_eq!(path.path, None);
    assert_eq!(path.scope.map(|s| (s.requested, s.in_scope)), Some((3, 3)));

    let partition: CommunityDetectionView = serde_json::from_value(json_of(
        served.read_as(TENANT, communities(Some(&["a", "b"]))).await,
    ))
    .unwrap();
    let mut members: Vec<_> = partition.communities.concat();
    members.sort();
    assert_eq!(members, ["a", "b"]);

    // An empty scope is a scope of nothing, not the whole graph.
    let nothing: PageRankView =
        serde_json::from_value(json_of(served.read_as(TENANT, page_rank(Some(&[]))).await))
            .unwrap();
    assert!(nothing.scores.is_empty());
    assert_eq!(
        nothing.scope,
        Some(AlgorithmScopeView {
            requested: 0,
            in_scope: 0
        })
    );
}

// spec: MEDULLA-FEDERATED-QUERY-R743.2
#[tokio::test(flavor = "multi_thread")]
async fn a_node_the_caller_cannot_see_answers_byte_identically_to_an_unknown_one() {
    let served = Served::seeded().await;
    for request in [page_rank, shortest_path, communities] {
        let hidden = served
            .read_as(TENANT, request(Some(&["a", "b", "x", "d"])))
            .await;
        let unknown = served
            .read_as(TENANT, request(Some(&["a", "b", "no-such-node", "d"])))
            .await;
        assert_eq!(
            serde_json::to_vec(&json_of(hidden)).unwrap(),
            serde_json::to_vec(&json_of(unknown)).unwrap(),
            "a hidden id and an unknown id must be indistinguishable"
        );
    }
    // The other tenant sees `x` and not the first tenant's chain.
    let own: PageRankView = serde_json::from_value(json_of(
        served
            .read_as(OTHER_TENANT, page_rank(Some(&["a", "x"])))
            .await,
    ))
    .unwrap();
    assert_eq!(
        own.scope,
        Some(AlgorithmScopeView {
            requested: 2,
            in_scope: 1
        })
    );
    assert_eq!(own.scores.len(), 1);
    assert_eq!(own.scores[0].node_id, "x");
}

// spec: MEDULLA-FEDERATED-QUERY-R743.4
#[tokio::test(flavor = "multi_thread")]
async fn a_scope_above_the_batch_bound_is_refused_whole() {
    let served = Served::seeded().await;
    let ids: Vec<String> = (0..=medulla_server_api::state::MAX_BATCH_IDS)
        .map(|i| format!("n{i}"))
        .collect();
    for method in ["PageRank", "GetShortestPath", "CommunityDetection"] {
        let mut params = match method {
            "PageRank" => serde_json::json!({"damping": 0.85, "iterations": 5}),
            "GetShortestPath" => serde_json::json!({"source_id": "a", "target_id": "d"}),
            _ => serde_json::json!({"resolution": 1.0}),
        };
        params["node_ids"] = serde_json::json!(ids);
        let response = served
            .read_as(
                TENANT,
                serde_json::json!({"method": method, "params": params}),
            )
            .await;
        let text = response.refusal_text().expect("a refusal");
        assert!(text.starts_with("INVALID_ARGUMENT"), "{method}: {text}");
        assert!(response.result.is_none(), "{method}: no partial result");
    }
}

// spec: MEDULLA-FEDERATED-QUERY-R743.7
#[tokio::test(flavor = "multi_thread")]
async fn a_graph_read_principal_is_allowed_a_scoped_call_and_refused_a_whole_graph_call() {
    let served = Served::seeded().await;
    let scopes = ["compute:graph-read"];
    for (name, scoped, whole) in [
        (
            "PageRank",
            page_rank(Some(&["a", "b", "c"])),
            page_rank(None),
        ),
        (
            "GetShortestPath",
            shortest_path(Some(&["a", "b", "c", "d"])),
            shortest_path(None),
        ),
        (
            "CommunityDetection",
            communities(Some(&["a", "b", "c"])),
            communities(None),
        ),
    ] {
        let allowed = served.read_with_scopes(TENANT, &scopes, scoped).await;
        assert!(
            allowed.error.is_none(),
            "{name} scoped refused: {:?}",
            allowed.refusal_text()
        );
        let refused = served.read_with_scopes(TENANT, &scopes, whole).await;
        let text = refused
            .refusal_text()
            .unwrap_or_else(|| panic!("{name} whole-graph call was allowed"));
        assert!(
            text.contains("ACCESS_DENIED") && text.contains("compute:graph-algo"),
            "{name}: {text}"
        );
    }
}
