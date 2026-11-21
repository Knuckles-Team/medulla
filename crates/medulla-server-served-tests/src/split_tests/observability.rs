//! MEDULLA-OBSERVE-R006.1, R003.3 and R007.1: the served `Introspect` read answers every
//! section by its refusals and pages, shows the lazy-paging task with its keyset cursor, and
//! shows a graph's readiness follow open, eviction and materialization.
//!
//! Every request goes through the real verified dispatch.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use medulla_identity_types::acl::RequestContextClaims;
use medulla_persistence::cold_offload::{lazy_open, DEFAULT_PRODUCTION_MAX_RESIDENT_GRAPHS};
use medulla_protocol_types::protocol::{GraphType, Method, Request, Response, ResultPayload};
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_server_api::state::ServerState;
use medulla_types::contract::Nonce;
use medulla_types::introspect::{IntrospectPage, IntrospectSection};
use tokio::sync::RwLock;

const TENANT: &str = "observe-tenant";
const PRINCIPAL: &str = "observe-owner";

fn next_id() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

struct Served {
    _dir: tempfile::TempDir,
    state: Arc<RwLock<ServerState>>,
}

impl Served {
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let mut state = medulla_server::seams_install::new_state_for_test(
            "observability-fixture-secret", // sanitizer:ignore
            ServerState::test_isolation(PRINCIPAL),
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
        let state = Arc::new(RwLock::new(state));
        medulla_server::seams_install::install_seams_on(&state).await;
        Self { _dir: dir, state }
    }

    /// One request as `PRINCIPAL` on `graph`, through the verified dispatch.
    async fn call(&self, graph: &str, method: Method) -> Response {
        let id = next_id();
        let claims = RequestContextClaims {
            principal: PRINCIPAL.to_string(),
            agent_id: PRINCIPAL.to_string(),
            tenant: TENANT.to_string(),
            audience: "medulla-test".to_string(),
            policy_version: "policy-test".to_string(),
            scopes: vec!["*".to_string()],
            ..RequestContextClaims::default()
        };
        let mut nonce = [0u8; 32];
        nonce[..8].copy_from_slice(&id.to_be_bytes());
        let context = VerifiedRequestContext::from_verified_claims_with_nonce(
            claims,
            format!("observe-{id}"),
            Some(Nonce::from_bytes(nonce)),
        );
        let request = Request {
            id,
            graph: graph.to_string(),
            auth_token: String::new(),
            agent_id: Some(PRINCIPAL.to_string()),
            method,
        };
        Box::pin(
            medulla_server_router::dispatch::request_boundary::dispatch_verified_request(
                &self.state,
                request,
                context,
            ),
        )
        .await
    }

    async fn introspect(
        &self,
        section: IntrospectSection,
        limit: Option<u16>,
        cursor: Option<&str>,
    ) -> Response {
        self.call(
            "__commons__",
            Method::Introspect {
                section,
                limit,
                cursor: cursor.map(str::to_string),
            },
        )
        .await
    }

    async fn page(&self, section: IntrospectSection) -> IntrospectPage {
        page_of("Introspect", self.introspect(section, None, None).await)
    }

    /// Create `graph`, write `nodes` nodes into it, evict it and open it lazily in pages of
    /// `page_size`; the first page is applied inline and a task pages the rest.
    async fn lazy_graph(&self, graph: &str, nodes: u64, page_size: usize) {
        let created = self
            .call(
                graph,
                Method::CreateGraph {
                    graph_name: graph.to_string(),
                    graph_type: GraphType::Agent,
                },
            )
            .await;
        assert!(created.error.is_none(), "create {graph}: {:?}", created.error);
        for i in 0..nodes {
            let added = self
                .call(
                    graph,
                    Method::AddNode {
                        node_id: format!("n{i}"),
                        properties_msgpack: rmp_serde::to_vec(&serde_json::json!({"payload": i}))
                            .unwrap(),
                    },
                )
                .await;
            assert!(added.error.is_none(), "add n{i}: {:?}", added.error);
        }
        assert!(self.state.write().await.registry.evict_resident(graph));
        assert!(
            lazy_open(
                &self.state,
                graph,
                DEFAULT_PRODUCTION_MAX_RESIDENT_GRAPHS,
                page_size
            )
            .await
        );
    }
}

fn page_of(label: &str, response: Response) -> IntrospectPage {
    assert!(
        response.error.is_none(),
        "{label} was refused: {response:?}"
    );
    match response.result {
        Some(ResultPayload::Raw(bytes)) => rmp_serde::from_slice(&bytes)
            .unwrap_or_else(|error| panic!("{label} result does not decode: {error}")),
        Some(ResultPayload::Json(value)) => serde_json::from_value(value)
            .unwrap_or_else(|error| panic!("{label} result does not decode: {error}")),
        other => panic!("{label} answered an unexpected payload shape: {other:?}"),
    }
}

fn refusal(response: Response) -> String {
    response.error.expect("the call must be refused")
}

/// Wait until the task named `name` is terminal, within a bounded deadline.
async fn finished_task(name: &str) -> medulla_observe::registry::tasks::TaskRow {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        if let Some(row) = medulla_observe::global()
            .get(name)
            .filter(|row| row.state.is_terminal())
        {
            return row;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "task {name} did not finish within the deadline"
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
}

// spec: MEDULLA-OBSERVE-R006.1
#[tokio::test(flavor = "multi_thread")]
async fn introspect_sections_answer_over_the_served_path() {
    let served = Served::new().await;
    for section in [
        IntrospectSection::Requests,
        IntrospectSection::Tasks,
        IntrospectSection::Runtime,
        IntrospectSection::Locks,
        IntrospectSection::Readiness,
        IntrospectSection::Startup,
        IntrospectSection::Build,
        IntrospectSection::Events,
    ] {
        let page = served.page(section).await;
        assert_eq!(page.section, section);
    }

    let limit = refusal(
        served
            .introspect(IntrospectSection::Tasks, Some(0), None)
            .await,
    );
    assert!(limit.starts_with("INTROSPECT_LIMIT_INVALID"), "{limit}");
    let limit = refusal(
        served
            .introspect(IntrospectSection::Tasks, Some(501), None)
            .await,
    );
    assert!(limit.starts_with("INTROSPECT_LIMIT_INVALID"), "{limit}");
    let cursor = refusal(
        served
            .introspect(IntrospectSection::Tasks, None, Some("not-a-task-id"))
            .await,
    );
    assert!(cursor.starts_with("INTROSPECT_CURSOR_INVALID"), "{cursor}");

    // Two registered tasks page ascending by id, one row per page.
    let registry = medulla_observe::global();
    let first = registry.register(medulla_observe::registry::tasks::TaskSpec {
        name: "introspect-served:first".to_string(),
        owner: "test".to_string(),
        kind: medulla_observe::registry::tasks::TaskKind::External,
    });
    let second = registry.register(medulla_observe::registry::tasks::TaskSpec {
        name: "introspect-served:second".to_string(),
        owner: "test".to_string(),
        kind: medulla_observe::registry::tasks::TaskKind::External,
    });
    let first_id = first.row().id;
    let page = page_of(
        "Introspect tasks",
        served
            .introspect(
                IntrospectSection::Tasks,
                Some(1),
                Some(&(first_id - 1).to_string()),
            )
            .await,
    );
    assert_eq!(page.rows.len(), 1);
    assert_eq!(page.rows[0]["name"], "introspect-served:first");
    assert_eq!(page.next_cursor.as_deref(), Some(first_id.to_string().as_str()));
    let next = page_of(
        "Introspect tasks",
        served
            .introspect(
                IntrospectSection::Tasks,
                Some(500),
                page.next_cursor.as_deref(),
            )
            .await,
    );
    assert!(next
        .rows
        .iter()
        .any(|row| row["name"] == "introspect-served:second"));
    assert!(next.seq >= page.seq);
    drop((first, second));
}

// spec: MEDULLA-OBSERVE-R003.3
#[tokio::test(flavor = "multi_thread")]
async fn lazy_paging_task_shows_cursor_in_introspect() {
    let _env_read_lock = medulla_kernel::crypto::acquire_test_env_read_lock().await;
    let served = Served::new().await;
    served.lazy_graph("observe:paged", 25, 5).await;
    let finished = finished_task("lazy-page-in:observe:paged").await;
    assert_eq!(
        finished.state,
        medulla_observe::machine::task::TaskState::Completed
    );

    let page = served.page(IntrospectSection::Tasks).await;
    let row = page
        .rows
        .iter()
        .find(|row| row["name"] == "lazy-page-in:observe:paged")
        .expect("the paging task is listed");
    assert_eq!(row["owner"], "registry");
    assert_eq!(row["kind"], "Materialization");
    assert_eq!(row["cursor"]["kind"], "keyset");
    assert!(row["cursor"]["node"]
        .as_str()
        .is_some_and(|node| node.starts_with('n')));
    // The first page was applied inline; the task paged the other twenty nodes.
    assert_eq!(row["units_done"], 20);
}

// spec: MEDULLA-OBSERVE-R007.1
#[tokio::test(flavor = "multi_thread")]
async fn graph_readiness_follows_open_migrate_materialize() {
    let _env_read_lock = medulla_kernel::crypto::acquire_test_env_read_lock().await;
    let served = Served::new().await;
    served.lazy_graph("observe:ready", 12, 4).await;
    finished_task("lazy-page-in:observe:ready").await;

    let page = served.page(IntrospectSection::Readiness).await;
    let row = page
        .rows
        .iter()
        .find(|row| row["graph"] == "observe:ready")
        .expect("the graph has a readiness row");
    assert_eq!(row["state"], "ready");
    assert!(row["reason_code"].is_null());

    // Evicting a ready graph reopens it: the row is back at opening until the next open.
    assert!(served.state.write().await.registry.evict_resident("observe:ready"));
    let page = served.page(IntrospectSection::Readiness).await;
    let row = page
        .rows
        .iter()
        .find(|row| row["graph"] == "observe:ready")
        .unwrap();
    assert_eq!(row["state"], "opening");
}
