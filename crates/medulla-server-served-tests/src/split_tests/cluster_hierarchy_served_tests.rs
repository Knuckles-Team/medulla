//! MEDULLA-FEDERATED-QUERY-R744: `ClusterHierarchyClusters` and `ClusterHierarchyExpand` answer the
//! published `ClusterLevelView` and `ClusterExpansion`, through the real dispatch chain over a
//! durable served engine (the hierarchy cache lives in the graph backend).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use medulla_protocol_types::protocol::{GraphType, Method, Response, ResultPayload};
use medulla_result_types::compute_result::algorithms::{
    ClusterExpansion, ClusterHierarchySummary, ClusterLevelView,
};
use medulla_server_api::state::ServerState;
use medulla_server_auth::auth::{dispatch_test_on_heap, scoped_test_request_on, ScopedTestCaller};
use tokio::sync::RwLock;

const SECRET: &str = "cluster-hierarchy-served-secret"; // sanitizer:ignore
const TENANT: &str = "tenant-shared";
const ADMIN: &str = "cluster-hierarchy-served-admin";
const COMMONS: &str = "__commons__";
const NODES: [&str; 6] = ["a0", "a1", "a2", "b0", "b1", "b2"];

fn next_id() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

struct Served {
    _dir: tempfile::TempDir,
    state: Arc<RwLock<ServerState>>,
}

impl Served {
    /// A durable served engine whose commons graph holds two triangles joined by one edge.
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let mut state = medulla_server::seams_install::new_state_for_test(
            SECRET,
            ServerState::test_isolation(ADMIN),
        );
        state.persist_dir = Some(dir.path().to_string_lossy().into_owned());
        let graphs = dir.path().join("graphs");
        std::fs::create_dir_all(&graphs).unwrap();
        state.persistence = Some(Arc::new(
            medulla_persistence::redb_backend::RedbBackend::open(
                graphs.to_string_lossy().into_owned(),
                64,
            )
            .expect("open the served graph backend"),
        ));
        let served = Self {
            _dir: dir,
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
        for id in NODES {
            let added = served
                .call(Method::AddNode {
                    node_id: id.to_string(),
                    properties_msgpack: rmp_serde::to_vec_named(&serde_json::json!({
                        "type": "Doc", "node_type": "Doc",
                    }))
                    .unwrap(),
                })
                .await;
            assert!(added.error.is_none(), "seed node: {:?}", added.error);
        }
        for (source, target) in [
            ("a0", "a1"),
            ("a1", "a2"),
            ("a0", "a2"),
            ("b0", "b1"),
            ("b1", "b2"),
            ("b0", "b2"),
            ("a2", "b0"),
        ] {
            let linked = served
                .call(Method::AddEdge {
                    source_id: source.to_string(),
                    target_id: target.to_string(),
                    properties_msgpack: rmp_serde::to_vec_named(&serde_json::json!({
                        "relationship": "LINKS",
                    }))
                    .unwrap(),
                })
                .await;
            assert!(linked.error.is_none(), "seed edge: {:?}", linked.error);
        }
        served
    }

    async fn call(&self, method: Method) -> Response {
        let call_id = next_id();
        let request = scoped_test_request_on(
            SECRET,
            call_id,
            COMMONS,
            method,
            ScopedTestCaller {
                principal: ADMIN,
                tenant: TENANT,
                scopes: &["*"],
                nonce: &format!("cluster-hierarchy-served-nonce-{call_id}"),
                idempotency_key: &format!("cluster-hierarchy-served-{call_id}"),
            },
        );
        dispatch_test_on_heap(&self.state, request).await
    }
}

fn view<T: serde::de::DeserializeOwned>(response: Response) -> T {
    assert!(response.error.is_none(), "refused: {:?}", response.error);
    match response.result {
        Some(ResultPayload::Json(value)) => serde_json::from_value(value).expect("a typed view"),
        other => panic!("expected a Json view, got {other:?}"),
    }
}

// spec: MEDULLA-FEDERATED-QUERY-R744.3
#[tokio::test(flavor = "multi_thread")]
async fn a_read_before_any_refresh_is_refused_and_names_the_refresh() {
    let served = Served::new().await;
    let response = served
        .call(Method::ClusterHierarchyClusters {
            level: 1,
            parent_cluster_id: None,
        })
        .await;
    let text = response.refusal_text().expect("a refusal");
    assert!(text.contains("ClusterHierarchyRefresh"), "{text}");
    assert!(response.result.is_none());
}

// spec: MEDULLA-FEDERATED-QUERY-R744.3
#[tokio::test(flavor = "multi_thread")]
async fn refresh_then_reads_answer_the_published_level_and_expansion_views() {
    let served = Served::new().await;
    let summary: ClusterHierarchySummary = view(
        served
            .call(Method::ClusterHierarchyRefresh {
                label: None,
                resolution: 1.0,
                seed: 7,
            })
            .await,
    );
    assert!(summary.cached);
    assert_eq!(summary.base_node_count, NODES.len());
    assert!(summary.levels >= 1);

    let level: ClusterLevelView = view(
        served
            .call(Method::ClusterHierarchyClusters {
                level: 1,
                parent_cluster_id: None,
            })
            .await,
    );
    assert_eq!(level.level, 1);
    assert_eq!(
        level.clusters.iter().map(|c| c.node_count).sum::<usize>(),
        NODES.len(),
        "level 1 is the finest level: every node is in exactly one cluster"
    );
    assert!(level
        .inter_cluster_edges
        .iter()
        .all(|edge| (edge.src_idx as usize) < level.clusters.len()
            && (edge.dst_idx as usize) < level.clusters.len()));

    let first = level.clusters[0].id.clone();
    let expansion: ClusterExpansion = view(
        served
            .call(Method::ClusterHierarchyExpand { cluster_id: first })
            .await,
    );
    assert!(
        expansion.child_clusters.is_empty(),
        "a level-1 cluster has no children"
    );
    assert!(!expansion.nodes.is_empty());
    assert!(expansion
        .nodes
        .iter()
        .all(|node| NODES.contains(&node.id.as_str())));

    let out_of_range = served
        .call(Method::ClusterHierarchyClusters {
            level: summary.levels + 1,
            parent_cluster_id: None,
        })
        .await;
    assert!(out_of_range
        .refusal_text()
        .expect("a refusal")
        .contains("out of range"));
}
