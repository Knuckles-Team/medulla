//! MEDULLA-IDENTITY-R727.1: no cross-tenant read, count, search or inference.
//!
//! Two tenants' nodes and embeddings live in ONE store. Each tenant's verified
//! context is dispatched through the real chokepoint (`dispatch_verified_request`),
//! so the read scope (`scope_view`) is what separates them. The door is checked
//! separately: a carrier signed for the other tenant is refused
//! `AUTH_TENANT_MISMATCH` before any handler.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use medulla_protocol_types::protocol::{GraphType, Method, Request, Response, ResultPayload};
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_server_api::state::ServerState;
use medulla_server_auth::auth::{dispatch_test_on_heap, scoped_test_request_on, ScopedTestCaller};
use tokio::sync::RwLock;

const SECRET: &str = "tenant-separation-secret"; // sanitizer:ignore
/// The fixed tenant the served test auth policy admits at the door.
const TENANT: &str = "tenant-shared";
/// A second tenant whose nodes live in the same store.
const OTHER_TENANT: &str = "tenant-other";
const ADMIN: &str = "tenant-separation-admin";
const COMMONS: &str = "__commons__";

fn next_id() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// The node-id prefix of each tenant's nodes.
fn prefix(tenant: &str) -> &'static str {
    if tenant == TENANT {
        "node-a-"
    } else {
        "node-b-"
    }
}

struct Served {
    _dir: tempfile::TempDir,
    state: Arc<RwLock<ServerState>>,
}

impl Served {
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
        for tenant in [TENANT, OTHER_TENANT] {
            for index in 0..3u32 {
                let id = format!("{}{index}", prefix(tenant));
                served.seed(&id, tenant, index).await;
            }
        }
        served
    }

    /// One node of `tenant` with `owner_tenant` and `_tenant` set, and one embedding.
    async fn seed(&self, id: &str, tenant: &str, index: u32) {
        let node = Method::AddNode {
            node_id: id.to_string(),
            properties_msgpack: rmp_serde::to_vec_named(&serde_json::json!({
                "type": "Doc", "node_type": "Doc", "text": id,
                "owner_tenant": tenant, "_tenant": tenant,
            }))
            .unwrap(),
        };
        let mut embedding = vec![0.0f32; 64];
        embedding[(index % 64) as usize] = 1.0;
        let embed = Method::AddEmbedding {
            node_id: id.to_string(),
            embedding,
        };
        for (step, method) in [node, embed].into_iter().enumerate() {
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
                    nonce: &format!("tenant-separation-nonce-{call_id}"),
                    idempotency_key: &format!("tenant-separation-seed-{step}-{call_id}"),
                },
            );
            let response = dispatch_test_on_heap(&self.state, request).await;
            assert!(
                response.error.is_none(),
                "seed failed: {:?}",
                response.error
            );
        }
    }

    /// `method` as `tenant`'s verified context, through the dispatch chokepoint.
    async fn read_as(&self, tenant: &str, method: Method) -> Response {
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

    async fn read_at_door(&self, tenant: &str, method: Method) -> Response {
        let call_id = next_id();
        let request = scoped_test_request_on(
            SECRET,
            call_id,
            COMMONS,
            method,
            ScopedTestCaller {
                principal: ADMIN,
                tenant,
                scopes: &["*"],
                nonce: &format!("tenant-separation-door-nonce-{call_id}"),
                idempotency_key: &format!("tenant-separation-door-{call_id}"),
            },
        );
        dispatch_test_on_heap(&self.state, request).await
    }
}

/// A method built from its wire form, so later optional fields keep the literal valid.
fn wire(method: &str, params: serde_json::Value) -> Method {
    serde_json::from_value(serde_json::json!({"method": method, "params": params}))
        .expect("a wire method")
}

fn uql(text: &str) -> Method {
    wire("Uql", serde_json::json!({"text": text}))
}

/// The answer of `response` as JSON text, whichever payload form carries it.
fn answer_text(label: &str, response: Response) -> String {
    assert!(
        response.error.is_none(),
        "{label} failed: {:?}",
        response.refusal_text()
    );
    match response.result {
        Some(ResultPayload::Raw(bytes)) => {
            let value: serde_json::Value =
                rmp_serde::from_slice(&bytes).expect("a typed MessagePack answer");
            value.to_string()
        }
        Some(other) => serde_json::to_string(&other).expect("a serializable answer"),
        None => String::new(),
    }
}

fn assert_only_own(label: &str, tenant: &str, text: &str) {
    let other = if tenant == TENANT {
        prefix(OTHER_TENANT)
    } else {
        prefix(TENANT)
    };
    assert!(!text.contains(other), "{label} of {tenant} names {other}*: {text}");
}

// spec: MEDULLA-IDENTITY-R727.1
#[tokio::test(flavor = "multi_thread")]
async fn each_tenant_reads_counts_and_searches_only_its_own_nodes() {
    let served = Served::new().await;
    for tenant in [TENANT, OTHER_TENANT] {
        let own = prefix(tenant);

        // UQL rows name only the tenant's own nodes, all three of them.
        let rows = answer_text("Uql", served.read_as(tenant, uql("MATCH (:Doc)")).await);
        assert_only_own("UQL rows", tenant, &rows);
        for index in 0..3 {
            assert!(rows.contains(&format!("{own}{index}")), "{rows}");
        }

        // The count is the tenant's three nodes, not six.
        let count = answer_text(
            "Uql count",
            served
                .read_as(
                    tenant,
                    uql("MATCH (:Doc) |> GROUP BY label AGGREGATE COUNT AS n"),
                )
                .await,
        );
        let counts: Vec<u64> = count
            .split(|c: char| !c.is_ascii_digit())
            .filter_map(|part| part.parse().ok())
            .collect();
        assert!(counts.contains(&3), "{count}");
        assert!(!counts.contains(&6), "the count spans tenants: {count}");

        // SPARQL rows and entailed triples name only the tenant's own nodes.
        let triples = answer_text(
            "Sparql",
            served
                .read_as(
                    tenant,
                    wire(
                        "Sparql",
                        serde_json::json!({"query": "SELECT ?s ?p ?o WHERE { ?s ?p ?o }"}),
                    ),
                )
                .await,
        );
        assert_only_own("SPARQL triples", tenant, &triples);

        // Semantic search hits are the tenant's own nodes.
        let hits = answer_text(
            "SemanticSearch",
            served
                .read_as(
                    tenant,
                    Method::SemanticSearch {
                        query_embedding: {
                            let mut query = vec![0.0f32; 64];
                            query[0] = 1.0;
                            query
                        },
                        n_results: 10,
                    },
                )
                .await,
        );
        assert_only_own("semantic search", tenant, &hits);
    }
}

// spec: MEDULLA-IDENTITY-R727.1
#[tokio::test(flavor = "multi_thread")]
async fn foreign_tenant_context_is_refused_at_the_door() {
    let served = Served::new().await;
    let refused = served
        .read_at_door(OTHER_TENANT, uql("MATCH (:Doc)"))
        .await;
    assert!(refused.error.is_some(), "expected a refusal");
    assert!(refused
        .refusal_text()
        .unwrap_or_default()
        .contains("AUTH_TENANT_MISMATCH"));
    assert!(refused.result.is_none(), "no row leaks past the door");
}
