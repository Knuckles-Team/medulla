//! MEDULLA-OBSERVE-R006.1 to R006.5, R003.3, R005.3, R007.1 to R007.3, R008.4 and R009.2: the
//! served `Introspect` read answers every section by its refusals and pages, shows the
//! lazy-paging task with its keyset cursor, shows a graph's readiness follow open, eviction and
//! materialization, and the `Health` report names the blocking graph and the reason.
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
    /// Fails the test when one of its requests was answered by an uncoded refusal.
    tap: crate::refusal_tap::RefusalTap,
    last_id: AtomicU64,
}

impl Served {
    async fn new() -> Self {
        Self::new_in(tempfile::tempdir().unwrap()).await
    }

    /// A served engine over the persist directory `dir`. A directory a former engine left is
    /// reopened: this is how a test restarts the engine.
    async fn new_in(dir: tempfile::TempDir) -> Self {
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
        Self {
            _dir: dir,
            state,
            tap: crate::refusal_tap::RefusalTap::new(),
            last_id: AtomicU64::new(0),
        }
    }

    /// Stop this engine and hand back its persist directory.
    fn into_dir(self) -> tempfile::TempDir {
        self._dir
    }

    /// One request as `PRINCIPAL` on `graph`, through the verified dispatch.
    async fn call(&self, graph: &str, method: Method) -> Response {
        self.call_as(graph, method, vec!["*".to_string()]).await
    }

    /// One request as `PRINCIPAL` holding exactly `scopes`.
    async fn call_as(&self, graph: &str, method: Method, scopes: Vec<String>) -> Response {
        self.call_with(graph, method, scopes, None).await
    }

    /// One request that carries the caller's `trace_id`.
    async fn call_traced(&self, graph: &str, method: Method, trace_id: &str) -> Response {
        self.call_with(graph, method, vec!["*".to_string()], Some(trace_id))
            .await
    }

    /// The id of the request this fixture sent last.
    fn last_request_id(&self) -> u64 {
        self.last_id.load(Ordering::Relaxed)
    }

    async fn call_with(
        &self,
        graph: &str,
        method: Method,
        scopes: Vec<String>,
        trace_id: Option<&str>,
    ) -> Response {
        let id = next_id();
        self.last_id.store(id, Ordering::Relaxed);
        self.tap.issue(id);
        let claims = RequestContextClaims {
            principal: PRINCIPAL.to_string(),
            agent_id: PRINCIPAL.to_string(),
            tenant: TENANT.to_string(),
            audience: "medulla-test".to_string(),
            policy_version: "policy-test".to_string(),
            scopes,
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
            trace_id: trace_id.map(str::to_string),
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
        assert!(
            created.error.is_none(),
            "create {graph}: {:?}",
            created.error
        );
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
    decoded(label, response)
}

/// The typed result of an answered call.
fn decoded<T: serde::de::DeserializeOwned>(label: &str, response: Response) -> T {
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
// Proves every Introspect section answers over the served path with its limit and cursor refusals.
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
    assert_eq!(
        page.next_cursor.as_deref(),
        Some(first_id.to_string().as_str())
    );
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
// Proves a graph's readiness follows open, materialization and eviction.
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
    assert!(served
        .state
        .write()
        .await
        .registry
        .evict_resident("observe:ready"));
    let page = served.page(IntrospectSection::Readiness).await;
    let row = page
        .rows
        .iter()
        .find(|row| row["graph"] == "observe:ready")
        .unwrap();
    assert_eq!(row["state"], "opening");
}

// spec: MEDULLA-OBSERVE-R006.2
// Proves an in-flight request is listed with its phase, age and an opaque principal reference.
#[tokio::test(flavor = "multi_thread")]
async fn in_flight_request_is_listed_with_phase_and_age() {
    let served = Served::new().await;
    // The Introspect call is itself in flight while it reads the registry.
    let page = served.page(IntrospectSection::Requests).await;
    let row = page
        .rows
        .iter()
        .find(|row| row["method"] == "Introspect")
        .expect("the reading request is listed");
    assert_eq!(row["phase"], "handling");
    assert_eq!(row["graph"], "__commons__");
    assert!(row["age_ms"].is_u64());
    assert!(row["waiting_for"].is_null());
    let principal_ref = row["principal_ref"].as_str().unwrap();
    assert!(principal_ref.starts_with("p:"));
    assert!(
        !principal_ref.contains(PRINCIPAL),
        "the principal is never listed"
    );
    // Every exit path removes the row: the request that just returned is gone.
    let request_id = row["request_id"].as_u64().unwrap();
    assert!(medulla_observe::requests()
        .snapshot()
        .iter()
        .all(|listed| listed.request_id != request_id));
}

// spec: MEDULLA-OBSERVE-R006.3
// Proves a worker that spins across two windows is flagged in the runtime section.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn spinning_worker_is_flagged_in_runtime_section() {
    let served = Served::new().await;
    // Three of the four workers spin for longer than two sampler windows; the fourth and
    // the test thread keep serving.
    let spinners: Vec<_> = (0..3)
        .map(|_| {
            tokio::spawn(async {
                let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
                while std::time::Instant::now() < until {
                    std::hint::spin_loop();
                }
            })
        })
        .collect();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(8);
    let flagged = loop {
        let page = served.page(IntrospectSection::Runtime).await;
        let spinning = page
            .rows
            .iter()
            .filter(|row| row["spinning"] == true)
            .count();
        if spinning > 0 {
            break page;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "no worker was flagged spinning: {:?}",
            page.rows
        );
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    };
    assert_eq!(flagged.rows[0]["key"], "global");
    assert!(flagged.rows[0]["global_queue_depth"].is_u64());
    let worker = flagged
        .rows
        .iter()
        .find(|row| row["spinning"] == true)
        .unwrap();
    assert!(worker["busy_ratio"].as_f64().unwrap() >= 0.9);
    assert!(worker["parks"].is_u64() && worker["busy_ms"].is_u64());
    for spinner in spinners {
        spinner.await.unwrap();
    }
}

// spec: MEDULLA-OBSERVE-R006.4
// Proves a held graph lock shows its holder and a waiter, and an idle site shows none.
#[tokio::test(flavor = "multi_thread")]
async fn held_graph_lock_shows_holder_and_waiter() {
    let served = Served::new().await;
    let graph = "observe:locked";
    let site = format!("lock_graph:{graph}");
    let held = medulla_server_api::routed_write_coalescer::lock_graph(graph).await;
    let waiter = tokio::spawn(async move {
        let guard = medulla_server_api::routed_write_coalescer::lock_graph(graph).await;
        drop(guard);
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let row = loop {
        let page = served.page(IntrospectSection::Locks).await;
        if let Some(row) = page
            .rows
            .iter()
            .find(|row| row["name"] == site.as_str() && row["waiters"] == 1)
        {
            break row.clone();
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the waiter never showed: {:?}",
            page.rows
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    };
    assert!(row["holder"].is_string(), "a held lock names its holder");
    assert!(row["hold_ms"].is_u64());
    drop(held);
    waiter.await.unwrap();
    let page = served.page(IntrospectSection::Locks).await;
    let row = page
        .rows
        .iter()
        .find(|row| row["name"] == site.as_str())
        .expect("the site stays listed");
    assert!(row["holder"].is_null());
    assert_eq!(row["waiters"], 0);
    assert!(row["longest_wait_ms"].as_u64().is_some());
}

/// The readiness report `Health` answers.
async fn health(served: &Served) -> medulla_protocol_types::result_contract::cluster::HealthReport {
    decoded("Health", served.call("__commons__", Method::Health).await)
}

// spec: MEDULLA-OBSERVE-R007.2
#[tokio::test(flavor = "multi_thread")]
async fn health_names_the_blocking_graph_and_reason() {
    use medulla_observe::machine::readiness::DegradeReason;
    let served = Served::new().await;
    let opening = "observe:health-opening";
    let degraded = "observe:health-degraded";
    medulla_observe::readiness().open(opening);
    medulla_observe::readiness()
        .degrade(
            degraded,
            DegradeReason {
                code: "NO_PROGRESS".to_string(),
                task: Some("lazy-page-in:observe:health-degraded".to_string()),
                detail: "the paging cursor stopped".to_string(),
            },
        )
        .expect("a new graph degrades from opening");

    let report = health(&served).await;
    let readiness = report.readiness.expect("Health carries readiness");
    assert!(
        !readiness.ready,
        "a blocked graph makes the engine not ready"
    );
    let blocker = |graph: &str| {
        readiness
            .blockers
            .iter()
            .find(|blocker| blocker.graph == graph)
            .unwrap_or_else(|| panic!("no blocker for {graph}"))
    };
    assert_eq!(blocker(opening).state, "opening");
    assert_eq!(blocker(opening).reason_code, "OPENING");
    assert_eq!(blocker(degraded).state, "degraded");
    assert_eq!(blocker(degraded).reason_code, "NO_PROGRESS");
    assert_eq!(
        blocker(degraded).task.as_deref(),
        Some("lazy-page-in:observe:health-degraded")
    );
    assert!(blocker(degraded).since_unix_ms > 0);
    // Every catalog graph the registry knows is listed with its state.
    assert!(readiness
        .graphs
        .iter()
        .any(|row| row.graph == degraded && row.reason_code.as_deref() == Some("NO_PROGRESS")));
    assert!(readiness.blockers.len() <= medulla_types::readiness_report::MAX_HEALTH_BLOCKERS);

    // A graph that finished materializing no longer blocks.
    medulla_observe::readiness()
        .apply(
            opening,
            medulla_observe::machine::readiness::ReadinessEvent::Materialized,
            None,
        )
        .expect("opening graph materializes");
    let report = health(&served).await;
    let readiness = report.readiness.unwrap();
    assert!(readiness
        .blockers
        .iter()
        .all(|blocker| blocker.graph != opening));
    medulla_observe::readiness().remove(opening);
    medulla_observe::readiness().remove(degraded);
}

// spec: MEDULLA-OBSERVE-R007.3
#[tokio::test(flavor = "multi_thread")]
async fn health_answers_degraded_when_state_lock_is_held() {
    let served = Served::new().await;
    let previous = medulla_observe::settings::health_deadline_ms();
    medulla_observe::settings::set_health_deadline_ms(100);
    // Hold the write lock through the instrumented site, so the lock registry names the holder.
    let held = medulla_server_api::state::timed_write(&served.state).await;
    let started = std::time::Instant::now();
    let response = medulla_server_router::dispatch::router::service_control::health_response(
        &served.state,
        next_id(),
    )
    .await;
    let waited = started.elapsed();
    drop(held);
    medulla_observe::settings::set_health_deadline_ms(previous);

    let report: medulla_protocol_types::result_contract::cluster::HealthReport =
        decoded("Health", response);
    let readiness = report.readiness.expect("the busy answer carries readiness");
    assert!(!readiness.ready);
    let blocker = readiness
        .blockers
        .iter()
        .find(|blocker| blocker.reason_code == "STATE_LOCK_BUSY")
        .expect("the busy state lock is a blocker");
    assert_eq!(blocker.graph, "", "the blocker belongs to the process");
    assert!(blocker.task.is_some(), "the holder of the lock is named");
    assert!(
        waited < std::time::Duration::from_secs(5),
        "the call answered at its deadline, not when the lock freed: {waited:?}"
    );
    // The report has no status field (MEDULLA-CONTRACT-R062.3): readiness is the signal.
    let value = serde_json::to_value(&readiness).unwrap();
    assert!(value.get("status").is_none());

    // With the lock free the same call answers the full report.
    let report = health(&served).await;
    assert!(report.readiness.is_some_and(|readiness| readiness
        .blockers
        .iter()
        .all(|blocker| blocker.reason_code != "STATE_LOCK_BUSY")));
}

// spec: MEDULLA-OBSERVE-R005.3
#[tokio::test(flavor = "multi_thread")]
async fn stalled_materialization_degrades_graph_readiness() {
    use medulla_observe::registry::tasks::{ProgressCursor, TaskKind, TaskRegistry, TaskSpec};
    let served = Served::new().await;
    let graph = "observe:stalled";
    medulla_observe::readiness().open(graph);
    // An isolated task registry keeps the scan to this test's own task; the readiness registry
    // is the process-wide one `Introspect` and `Health` read.
    let tasks = TaskRegistry::new(8);
    let paging = tasks.register(TaskSpec {
        name: format!("lazy-page-in:{graph}"),
        owner: "registry".to_string(),
        kind: TaskKind::Materialization,
    });
    paging.progress(ProgressCursor::Counter(1), 1);
    let progressed = paging.row().last_progress_unix_ms;
    let stalled = medulla_observe::watchdog::scan(
        &tasks,
        medulla_observe::readiness(),
        progressed + 10_001,
        10_000,
    );
    assert_eq!(stalled.len(), 1);
    assert_eq!(stalled[0].degraded_graph.as_deref(), Some(graph));

    let page = served.page(IntrospectSection::Readiness).await;
    let row = page
        .rows
        .iter()
        .find(|row| row["graph"] == graph)
        .expect("the graph has a readiness row");
    assert_eq!(row["state"], "degraded");
    assert_eq!(row["reason_code"], "NO_PROGRESS");
    assert_eq!(row["reason_task"], format!("lazy-page-in:{graph}"));
    let report = health(&served).await.readiness.unwrap();
    assert!(!report.ready);
    assert!(report.blockers.iter().any(|blocker| blocker.graph == graph
        && blocker.task.as_deref() == Some(format!("lazy-page-in:{graph}").as_str())));
    medulla_observe::readiness().remove(graph);
    drop(paging);
}

// spec: MEDULLA-OBSERVE-R008.4
#[tokio::test(flavor = "multi_thread")]
async fn startup_report_lists_every_step_with_outcome() {
    let served = Served::new().await;
    let graph = "observe:startup";
    medulla_observe::startup().step("test_step_ok", graph).ok();
    medulla_observe::startup()
        .step("test_step_skipped", graph)
        .skipped("nothing to do");
    medulla_observe::startup()
        .step("test_step_refused", graph)
        .refused("MIGRATION_REFUSED");
    medulla_observe::startup()
        .step("test_step_failed", graph)
        .failed("MATVIEW_RELOAD_FAILED");

    let page = served.page(IntrospectSection::Startup).await;
    let mine: Vec<_> = page
        .rows
        .iter()
        .filter(|row| row["graph"] == graph)
        .collect();
    assert_eq!(mine.len(), 4);
    let outcome = |name: &str| {
        let row = mine
            .iter()
            .find(|row| row["name"] == name)
            .unwrap_or_else(|| panic!("step {name} is not listed"));
        (
            row["outcome"].as_str().unwrap().to_string(),
            row["detail"].as_str().map(str::to_string),
        )
    };
    assert_eq!(outcome("test_step_ok"), ("Ok".to_string(), None));
    assert_eq!(
        outcome("test_step_skipped"),
        ("Skipped".to_string(), Some("nothing to do".to_string()))
    );
    assert_eq!(
        outcome("test_step_refused"),
        ("Refused".to_string(), Some("MIGRATION_REFUSED".to_string()))
    );
    assert_eq!(
        outcome("test_step_failed"),
        (
            "Failed".to_string(),
            Some("MATVIEW_RELOAD_FAILED".to_string())
        )
    );
    // The page is ascending by start time: the steps of the whole page never go backwards.
    let starts: Vec<u64> = page
        .rows
        .iter()
        .map(|row| row["started_unix_ms"].as_u64().unwrap())
        .collect();
    assert!(starts.windows(2).all(|pair| pair[0] <= pair[1]));
    assert!(page.rows.iter().all(|row| row["duration_ms"].is_u64()));
}

// spec: MEDULLA-OBSERVE-R009.2
#[tokio::test(flavor = "multi_thread")]
async fn build_section_reports_version_sha_build_id_features() {
    let served = Served::new().await;
    let page = served.page(IntrospectSection::Build).await;
    assert_eq!(page.rows.len(), 1);
    let row = &page.rows[0];
    let info = &*medulla_version::BUILD_INFO;
    assert_eq!(row["key"], "build");
    assert_eq!(row["version"], info.version);
    assert_eq!(row["git_sha"], info.git_sha);
    assert_eq!(row["profile"], info.profile);
    assert_eq!(row["build_id"].as_str().map(str::to_string), info.build_id);
    let features: Vec<&str> = row["features"]
        .as_array()
        .expect("the feature list")
        .iter()
        .map(|feature| feature.as_str().unwrap())
        .collect();
    assert_eq!(features, info.features);
    // The startup log line names the same build: it runs without a subscriber and without panic.
    medulla_h_admin::admin::introspect::log_build_identity();
}

// spec: MEDULLA-OBSERVE-R006.5
#[tokio::test(flavor = "multi_thread")]
async fn introspect_rows_carry_no_token_or_payload() {
    let served = Served::new().await;
    let graph = "observe:private";
    let created = served
        .call(
            graph,
            Method::CreateGraph {
                graph_name: graph.to_string(),
                graph_type: GraphType::Agent,
            },
        )
        .await;
    assert!(created.error.is_none(), "{:?}", created.error);
    let added = served
        .call(
            graph,
            Method::AddNode {
                node_id: "private-node".to_string(),
                properties_msgpack: rmp_serde::to_vec(
                    &serde_json::json!({"secret_field": "PAYLOAD-MARKER-4f1c"}),
                )
                .unwrap(),
            },
        )
        .await;
    assert!(added.error.is_none(), "{:?}", added.error);
    let catalog_before = served.state.read().await.registry.catalog_len();

    // Every section, whole, as text: none holds the principal, the signing secret, a payload
    // value or the name of a property.
    let mut text = String::new();
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
        let page = page_of(
            "Introspect",
            served.introspect(section, Some(500), None).await,
        );
        text.push_str(&serde_json::to_string(&page.rows).unwrap());
    }
    for forbidden in [
        PRINCIPAL,
        "observability-fixture-secret",
        "PAYLOAD-MARKER-4f1c",
        "secret_field",
        "private-node",
    ] {
        assert!(
            !text.contains(forbidden),
            "an Introspect row holds `{forbidden}`"
        );
    }
    // A caller without `observe:read` is refused and the refusal names no graph.
    let denied = served
        .call_as(
            "__commons__",
            Method::Introspect {
                section: IntrospectSection::Readiness,
                limit: None,
                cursor: None,
            },
            vec!["node:read".to_string()],
        )
        .await;
    let denial = denied
        .error
        .expect("a caller without observe:read is refused");
    assert!(
        !denial.contains(graph),
        "the refusal names a graph: {denial}"
    );
    // The call writes nothing: the catalog is unchanged.
    assert_eq!(
        served.state.read().await.registry.catalog_len(),
        catalog_before
    );
}

// spec: MEDULLA-OBSERVE-R001.3
#[tokio::test(flavor = "multi_thread")]
async fn collapsed_refusal_fails_the_harness() {
    use crate::refusal_tap::RefusalTap;
    let served = Served::new().await;

    // A typed refusal through the served path is never a collapse.
    let refused = served
        .introspect(IntrospectSection::Tasks, Some(0), None)
        .await;
    assert!(refusal(refused).starts_with("INTROSPECT_LIMIT_INVALID"));
    served.tap.assert_no_collapse();

    // A handler error without a code answers INTERNAL and the tap sees it for its own request.
    let text = "graph shard 9 lock poisoned: observe harness fixture";
    let tap = RefusalTap::new();
    let id = next_id();
    tap.issue(id);
    let response = Response::err(id, text);
    assert_eq!(response.error.as_deref(), Some("INTERNAL"));
    let collapsed = tap.collapsed();
    assert_eq!(collapsed.len(), 1);
    assert_eq!(collapsed[0].request_id, id);
    assert!(
        collapsed[0]
            .module
            .starts_with("medulla-server-served-tests"),
        "the reporting crate is named: {}",
        collapsed[0].module
    );
    // The harness fails the test at its end: the check panics while the collapse is undeclared.
    let failed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        tap.assert_no_collapse();
    }));
    assert!(
        failed.is_err(),
        "an undeclared collapse must fail the harness"
    );
    // A request the tap did not issue is none of its business: parallel tests stay independent.
    let other = Response::err(next_id(), "another test's uncoded refusal");
    assert_eq!(other.error.as_deref(), Some("INTERNAL"));
    assert_eq!(tap.collapsed().len(), 1);
    // Declaring the text makes the collapse expected, and the drop at the end of the test passes.
    tap.expect_unclassified(text);
    assert!(tap.collapsed().is_empty());
    // The series counts the collapse by the reporting crate.
    assert!(medulla_protocol_types::protocol::unclassified_by_module()
        .iter()
        .any(|(module, count)| module.starts_with("medulla-server-served-tests") && *count >= 2));
}

const TRACE_HEX: &str = "0af7651916cd43dd8448eb211c80319c";
const TRACEPARENT: &str = "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01";

// spec: MEDULLA-OBSERVE-R002.3
#[tokio::test(flavor = "multi_thread")]
async fn engine_log_line_inside_request_shows_ids() {
    let capture = crate::log_capture::install();
    let served = Served::new().await;

    // A request that carries a traceparent: every engine log line it prints runs inside the
    // request span with the request id, the trace id, the operation and the graph.
    let page = served
        .call_traced(
            "__commons__",
            Method::Introspect {
                section: IntrospectSection::Tasks,
                limit: None,
                cursor: None,
            },
            TRACEPARENT,
        )
        .await;
    assert!(page.error.is_none(), "{:?}", page.error);
    let id = served.last_request_id().to_string();
    let mine: Vec<_> = capture
        .events()
        .into_iter()
        .filter(|event| event.span_with("request_id", &id).is_some())
        .collect();
    let transitions: Vec<_> = mine
        .iter()
        .filter(|event| {
            event.target == "medulla::transition"
                && event.field("machine") == Some("RequestMachine")
        })
        .collect();
    assert!(
        transitions.len() >= 3,
        "the request's machine transitions were logged inside its span: {mine:#?}"
    );
    for event in &transitions {
        let span = event.span_with("request_id", &id).unwrap();
        assert_eq!(span.name, "request");
        assert_eq!(span.field("trace_id"), Some(TRACE_HEX));
        assert_eq!(span.field("op"), Some("Introspect"));
        assert_eq!(span.field("graph"), Some("__commons__"));
    }

    // Without a trace id, and with a value that is not a traceparent, the request runs and its
    // span shows an empty trace id: a malformed value is dropped, never refused.
    for (label, trace) in [("none", None), ("malformed", Some("not-a-traceparent"))] {
        let response = match trace {
            Some(trace) => {
                served
                    .call_traced(
                        "__commons__",
                        Method::Introspect {
                            section: IntrospectSection::Locks,
                            limit: None,
                            cursor: None,
                        },
                        trace,
                    )
                    .await
            }
            None => {
                served
                    .introspect(IntrospectSection::Locks, None, None)
                    .await
            }
        };
        assert!(response.error.is_none(), "{label}: {:?}", response.error);
        let id = served.last_request_id().to_string();
        let span = capture
            .events()
            .into_iter()
            .find_map(|event| event.span_with("request_id", &id).cloned())
            .unwrap_or_else(|| panic!("{label}: the request logged nothing inside its span"));
        assert_eq!(span.field("trace_id"), Some(""), "{label}");
    }
}

// spec: MEDULLA-OBSERVE-R002.4
#[tokio::test(flavor = "multi_thread")]
async fn one_trace_id_follows_a_request_into_its_spawned_task() {
    let capture = crate::log_capture::install();
    let _env_lock = medulla_kernel::crypto::acquire_test_env_lock().await;
    let served = Served::new().await;
    let graph = "observe:traced";
    let created = served
        .call(
            graph,
            Method::CreateGraph {
                graph_name: graph.to_string(),
                graph_type: GraphType::Agent,
            },
        )
        .await;
    assert!(created.error.is_none(), "{:?}", created.error);
    for i in 0..20u64 {
        let added = served
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
    assert!(served.state.write().await.registry.evict_resident(graph));

    // A read of the evicted graph opens it lazily in pages of four; the first page is applied
    // inline and a registered task pages the rest, spawned by this very request.
    std::env::set_var("MEDULLA_LAZY_OPEN_PAGE_SIZE", "4");
    let read = served
        .call_traced(
            graph,
            Method::GetNodeProperties {
                node_id: "n0".to_string(),
            },
            TRACEPARENT,
        )
        .await;
    std::env::remove_var("MEDULLA_LAZY_OPEN_PAGE_SIZE");
    assert!(read.error.is_none(), "{:?}", read.error);
    let id = served.last_request_id();

    let name = format!("lazy-page-in:{graph}");
    let row = finished_task(&name).await;
    assert_eq!(
        row.origin_request,
        Some(id),
        "the task records the request that spawned it"
    );
    let id_text = id.to_string();
    let inside_task: Vec<_> = capture
        .events()
        .into_iter()
        .filter(|event| {
            event.target == "medulla::transition"
                && event.field("machine") == Some("TaskMachine")
                && event.field("subject") == Some(name.as_str())
        })
        .collect();
    assert!(
        !inside_task.is_empty(),
        "the task's transitions were logged"
    );
    for event in &inside_task {
        let span = event
            .span_with("request_id", &id_text)
            .expect("a task line runs inside the request's span");
        assert_eq!(span.field("trace_id"), Some(TRACE_HEX));
    }
    // The task's introspection row names the origin request too.
    let page = served.page(IntrospectSection::Tasks).await;
    let listed = page
        .rows
        .iter()
        .find(|row| row["name"] == name.as_str())
        .expect("the finished task is listed");
    assert_eq!(listed["origin_request"], id);
}

/// Serializes the tests that start the boot maintenance: the task names it registers are unique
/// in the process-wide registry, so two boots at once would not both be listed.
static BOOT_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

// spec: MEDULLA-OBSERVE-R003.5
#[tokio::test(flavor = "multi_thread")]
async fn namespace_migration_task_and_steps_are_visible() {
    let _boot = BOOT_LOCK.lock().await;
    let _env_read_lock = medulla_kernel::crypto::acquire_test_env_read_lock().await;
    let served = Served::new().await;
    let graph = "observe:nsmig";
    let created = served
        .call(
            graph,
            Method::CreateGraph {
                graph_name: graph.to_string(),
                graph_type: GraphType::Agent,
            },
        )
        .await;
    assert!(created.error.is_none(), "{:?}", created.error);

    medulla_h_admin::boot_maintenance::spawn_post_bind_maintenance(served.state.clone()).await;
    let row = finished_task(medulla_h_admin::namespace_migration::TASK_NAME).await;
    assert_eq!(
        row.kind,
        medulla_observe::registry::tasks::TaskKind::Migration
    );
    assert_eq!(row.owner, "h-admin");
    // The graph is the task's cursor: the last graph the pass visited.
    assert!(matches!(
        row.cursor,
        Some(medulla_observe::registry::tasks::ProgressCursor::Opaque(_))
    ));
    assert!(row.units_done >= 1);

    // The pass recorded one startup step for the graph, with its outcome, and the page lists it.
    let page = served.page(IntrospectSection::Startup).await;
    let step = page
        .rows
        .iter()
        .find(|row| row["name"] == "namespace_migration" && row["graph"] == graph)
        .expect("the migration records one step for the graph");
    assert_eq!(step["outcome"], "Skipped");
    assert_eq!(step["detail"], "no retired namespace stored");
    let listed = served.page(IntrospectSection::Tasks).await;
    assert!(listed.rows.iter().any(|row| row["name"]
        == medulla_h_admin::namespace_migration::TASK_NAME
        && row["kind"] == "Migration"));
}

// spec: MEDULLA-OBSERVE-R008.3
#[tokio::test(flavor = "multi_thread")]
async fn refused_commons_migration_is_recorded_and_degrades_readiness() {
    let served = Served::new().await;
    let graph = "observe:refused-migration";
    let created = served
        .call(
            graph,
            Method::CreateGraph {
                graph_name: graph.to_string(),
                graph_type: GraphType::Agent,
            },
        )
        .await;
    assert!(created.error.is_none(), "{:?}", created.error);
    assert!(served.state.write().await.registry.evict_resident(graph));
    {
        // An identity policy that awaits its System bootstrap refuses the migration service's
        // grant on the graph, so opening the cold graph for migration is refused.
        let mut state = served.state.write().await;
        state.local_anonymous = false;
        state.isolation = medulla_isolation::isolation::IsolationLayer::new();
    }

    let report = medulla_h_admin::namespace_migration::run(&served.state).await;
    let refusal = report
        .refusals
        .iter()
        .find(|refusal| refusal.graph == graph)
        .expect("the cold graph's migration is refused");
    assert_eq!(refusal.code, "NAMESPACE_MIGRATION_POLICY_UNBOOTSTRAPPED");
    assert!(refusal
        .detail
        .contains("NAMESPACE_MIGRATION_POLICY_UNBOOTSTRAPPED"));

    // The step names the graph and ends Refused with the typed code.
    let step = medulla_observe::startup()
        .steps()
        .into_iter()
        .find(|step| step.name == "namespace_migration" && step.graph == graph)
        .expect("the refused graph records its step");
    assert_eq!(
        step.outcome,
        medulla_observe::startup::StepOutcome::Refused(
            "NAMESPACE_MIGRATION_POLICY_UNBOOTSTRAPPED".to_string()
        )
    );
    // The graph is degraded with the reason MIGRATION_REFUSED and names the migration task.
    let readiness = medulla_observe::readiness()
        .get(graph)
        .expect("a readiness row");
    assert_eq!(
        readiness.state,
        medulla_observe::machine::readiness::ReadinessState::Degraded
    );
    let reason = readiness.reason.expect("a degraded graph has a reason");
    assert_eq!(reason.code, "MIGRATION_REFUSED");
    assert_eq!(
        reason.task.as_deref(),
        Some(medulla_h_admin::namespace_migration::TASK_NAME)
    );
    medulla_observe::readiness().remove(graph);
}

// spec: MEDULLA-OBSERVE-R003.6
#[tokio::test(flavor = "multi_thread")]
async fn every_inventoried_background_task_is_registered_after_boot() {
    let _boot = BOOT_LOCK.lock().await;
    let _env_read_lock = medulla_kernel::crypto::acquire_test_env_read_lock().await;
    let served = Served::new().await;
    // The closed inventory of background tasks the boot starts, with the build condition under
    // which each exists. A new background task joins this list in the change that registers it.
    let inventory: [(&str, bool); 7] = [
        ("namespace-migration", true),
        ("matview-reload", cfg!(feature = "matview")),
        (
            "connector-pack-projection",
            cfg!(all(feature = "redb", feature = "blob", feature = "shacl")),
        ),
        (
            "connector-pack-projection-native",
            cfg!(all(feature = "redb", feature = "blob", feature = "shacl")),
        ),
        (
            "agent-library-projection",
            cfg!(all(feature = "redb", feature = "rdf")),
        ),
        (
            "agent-library-components",
            cfg!(all(feature = "redb", feature = "rdf")),
        ),
        ("reasoning-projection", cfg!(feature = "redb")),
    ];
    medulla_h_admin::boot_maintenance::spawn_post_bind_maintenance(served.state.clone()).await;
    medulla_h_admin::connector_pack_projection::spawn(served.state.clone());
    medulla_h_admin::agent_library_projection::spawn(served.state.clone());
    medulla_h_retrieve::reasoning_projection::spawn(served.state.clone());

    let page = served
        .introspect(IntrospectSection::Tasks, Some(500), None)
        .await;
    let page = page_of("Introspect tasks", page);
    for (name, built) in inventory {
        if !built {
            continue;
        }
        let row = page
            .rows
            .iter()
            .find(|row| row["name"] == name)
            .unwrap_or_else(|| panic!("background task `{name}` is not registered after boot"));
        assert!(
            row["owner"].as_str().is_some_and(|owner| !owner.is_empty()),
            "`{name}` names its owner"
        );
        assert!(
            row["registered_location"]
                .as_str()
                .is_some_and(|at| at.contains(".rs:")),
            "`{name}` records where it was registered"
        );
    }
}

// spec: MEDULLA-OBSERVE-R003.4
#[tokio::test(flavor = "multi_thread")]
async fn index_build_task_shows_cursor_in_introspect() {
    let served = Served::new().await;
    let graph = "observe:ann-index";
    let created = served
        .call(
            graph,
            Method::CreateGraph {
                graph_name: graph.to_string(),
                graph_type: GraphType::Agent,
            },
        )
        .await;
    assert!(created.error.is_none(), "{:?}", created.error);
    // Enough embeddings that the graph qualifies for an ANN index build.
    {
        let state = served.state.read().await;
        let entry = state
            .registry
            .all_entries()
            .into_iter()
            .find(|entry| entry.name == graph)
            .expect("the graph is resident");
        let mut store = entry.core.semantic_store.write();
        for i in 0..(medulla_semantic_index::semantic_ann::ANN_BUILD_THRESHOLD + 64) {
            let vector: Vec<f32> = (0..8)
                .map(|axis| ((i * (axis + 3)) % 17) as f32 + 1.0)
                .collect();
            store
                .add_embedding(format!("n{i}"), vector)
                .expect("an embedding of the store's width");
        }
    }
    let started =
        medulla_server_graph::semantic_activation::sweep_resident_graphs_with(&served.state, None)
            .await;
    assert!(started >= 1, "the sweep started a build for the graph");

    let name = medulla_server_graph::semantic_activation::ann_build_task_name(graph);
    let finished = finished_task(&name).await;
    assert_eq!(
        finished.kind,
        medulla_observe::registry::tasks::TaskKind::Index
    );
    assert_eq!(finished.owner, "semantic-index");
    let page = served.page(IntrospectSection::Tasks).await;
    let row = page
        .rows
        .iter()
        .find(|row| row["name"] == name.as_str())
        .expect("the build is listed");
    assert_eq!(row["kind"], "Index");
    assert_eq!(row["cursor"]["kind"], "counter");
    assert!(row["cursor"]["value"].is_u64());
}

/// Wait, within a bounded deadline, until `condition` holds.
async fn eventually(what: &str, mut condition: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while !condition() {
        assert!(
            std::time::Instant::now() < deadline,
            "{what} did not happen within the deadline"
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}

// spec: MEDULLA-OBSERVE-R008.2
#[tokio::test(flavor = "multi_thread")]
async fn previous_boot_report_is_queryable_after_restart() {
    let _boot = BOOT_LOCK.lock().await;
    let _env_read_lock = medulla_kernel::crypto::acquire_test_env_read_lock().await;

    // First boot: one recorded step, boot tasks run, and the report is stored when they end.
    let first = Served::new().await;
    let graph = "observe:restart";
    medulla_observe::startup()
        .step("restart_probe_first", graph)
        .ok();
    medulla_h_admin::boot_maintenance::spawn_post_bind_maintenance(first.state.clone()).await;
    let store = {
        let state = first.state.read().await;
        medulla_persistence::observe_startup_store::shared_observe_startup_store(&state)
            .expect("the startup report store opens under the persist directory")
    };
    eventually("the first boot's report was stored", || {
        store.list().is_ok_and(|boots| !boots.is_empty())
    })
    .await;
    let first_seq = store.list().unwrap().last().unwrap().boot_seq;
    drop(store);
    // The engine is stopped once no boot task holds its state any longer.
    eventually("the first engine's boot tasks released its state", || {
        Arc::strong_count(&first.state) == 1
    })
    .await;

    // Restart on the same directory: the engine loads the stored reports before it records any
    // step of the new boot.
    let dir = first.into_dir();
    let second = Served::new_in(dir).await;
    medulla_observe::startup()
        .step("restart_probe_second", graph)
        .ok();
    medulla_h_admin::boot_maintenance::spawn_post_bind_maintenance(second.state.clone()).await;

    let mut rows = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let page = page_of(
            "Introspect startup",
            second
                .introspect(IntrospectSection::Startup, Some(500), cursor.as_deref())
                .await,
        );
        rows.extend(page.rows);
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    let probe = |name: &str, boot_seq: u64| {
        rows.iter()
            .any(|row| row["name"] == name && row["graph"] == graph && row["boot_seq"] == boot_seq)
    };
    // The earlier boot's report answers under its own sequence number and is not the current one.
    assert!(
        probe("restart_probe_first", first_seq),
        "the previous report is queryable"
    );
    assert!(!probe("restart_probe_second", first_seq));
    // The new boot is the next sequence number and carries both probes in this process.
    let current = first_seq + 1;
    assert_eq!(medulla_observe::startup().boot_seq(), current);
    assert!(probe("restart_probe_second", current));
    assert!(rows
        .iter()
        .filter(|row| row["boot_seq"] == first_seq)
        .all(|row| row["current"] == false));
    assert!(rows
        .iter()
        .filter(|row| row["boot_seq"] == current)
        .all(|row| row["current"] == true));
}

/// The report a `Profile` call answered.
fn profile_report(label: &str, response: Response) -> medulla_types::introspect::ProfileReport {
    decoded(label, response)
}

// spec: MEDULLA-OBSERVE-R010.1
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn profile_samples_the_spinning_worker() {
    use medulla_types::introspect::ProfileKind;
    let served = Served::new().await;
    let profile = |kind, seconds| Method::Profile { kind, seconds };

    // A duration outside 1 to 60 is refused before anything is sampled.
    for seconds in [0u16, 61] {
        let refused = refusal(
            served
                .call("__commons__", profile(ProfileKind::Stacks, seconds))
                .await,
        );
        assert!(refused.starts_with("PROFILE_DURATION_INVALID"), "{refused}");
    }
    // A caller without `observe:profile` is refused.
    let denied = served
        .call_as(
            "__commons__",
            profile(ProfileKind::TaskDump, 0),
            vec!["observe:read".to_string()],
        )
        .await;
    assert!(
        denied.error.is_some(),
        "observe:read does not clear observe:profile"
    );

    if !medulla_observe::profile::available() {
        let unavailable = refusal(
            served
                .call("__commons__", profile(ProfileKind::Stacks, 1))
                .await,
        );
        assert!(
            unavailable.starts_with("PROFILE_UNAVAILABLE"),
            "{unavailable}"
        );
        return;
    }
    // Three workers spin while the sampler runs; the fourth and the test keep serving.
    let spinners: Vec<_> = (0..3)
        .map(|_| {
            tokio::spawn(async {
                let until = std::time::Instant::now() + std::time::Duration::from_secs(3);
                while std::time::Instant::now() < until {
                    std::hint::spin_loop();
                }
            })
        })
        .collect();
    let report = profile_report(
        "Profile stacks",
        served
            .call("__commons__", profile(ProfileKind::Stacks, 1))
            .await,
    );
    assert_eq!(report.kind, ProfileKind::Stacks);
    assert_eq!(report.seconds, 1);
    assert!(report.samples > 0, "threads were sampled");
    assert!(
        report
            .stacks
            .iter()
            .any(|stack| stack.thread.starts_with("tokio-runtime")),
        "the runtime workers are named by thread: {:?}",
        report
            .stacks
            .iter()
            .map(|stack| &stack.thread)
            .collect::<Vec<_>>()
    );
    // Every frame names its module and an offset in it.
    assert!(report
        .stacks
        .iter()
        .flat_map(|stack| &stack.frames)
        .all(|frame| !frame.module.is_empty()));
    for spinner in spinners {
        spinner.await.unwrap();
    }
}

// spec: MEDULLA-OBSERVE-R010.1
#[tokio::test(flavor = "multi_thread")]
async fn observe_scopes_are_operator_only_at_the_dispatch_chokepoint() {
    use medulla_types::introspect::ProfileKind;
    let served = Served::new().await;
    let profile = || Method::Profile {
        kind: ProfileKind::TaskDump,
        seconds: 0,
    };
    let introspect = || Method::Introspect {
        section: IntrospectSection::Readiness,
        limit: None,
        cursor: None,
    };
    // A plain read bearer reaches neither the sampler nor the registry dump.
    for method in [profile(), introspect()] {
        let denied = served
            .call_as("__commons__", method, vec!["kg:read".to_string()])
            .await;
        assert!(denied.error.is_some(), "a kg:read bearer was admitted");
    }
    // The admin aggregate is admitted: the call passes the scope gate and is
    // answered by the handler itself.
    let admitted = served
        .call_as("__commons__", introspect(), vec!["kg:admin".to_string()])
        .await;
    assert!(admitted.error.is_none(), "{:?}", admitted.error);
    let admitted = served
        .call_as("__commons__", profile(), vec!["kg:admin".to_string()])
        .await;
    let error = admitted.error.unwrap_or_default();
    assert!(
        !error.contains("ACCESS_DENIED") && !error.contains("scope"),
        "an admin was refused by the scope gate: {error}"
    );
}

// spec: MEDULLA-OBSERVE-R010.3
#[tokio::test(flavor = "multi_thread")]
async fn task_dump_is_one_consistent_snapshot() {
    use medulla_observe::registry::tasks::{TaskKind, TaskSpec};
    use medulla_types::introspect::ProfileKind;
    let served = Served::new().await;
    let dumped = medulla_observe::global().register(TaskSpec {
        name: "task-dump:served".to_string(),
        owner: "test".to_string(),
        kind: TaskKind::External,
    });
    let held = medulla_server_api::routed_write_coalescer::lock_graph("observe:dump-lock").await;
    let dump = |served: &Served| {
        served.call(
            "__commons__",
            Method::Profile {
                kind: ProfileKind::TaskDump,
                seconds: 0,
            },
        )
    };

    let first = profile_report("Profile task_dump", dump(&served).await);
    assert_eq!(first.kind, ProfileKind::TaskDump);
    // The four registries are listed by one read each, under one sequence number.
    assert!(first.seq > 0);
    assert!(first
        .tasks
        .iter()
        .any(|row| row["name"] == "task-dump:served"));
    assert!(
        first.requests.iter().any(|row| row["method"] == "Profile"),
        "the dump request itself is in flight"
    );
    assert!(first
        .locks
        .iter()
        .any(|row| row["name"] == "lock_graph:observe:dump-lock" && row["holder"].is_string()));
    assert!(first.events.len() <= medulla_observe::profile::TASK_DUMP_EVENTS);
    assert!(first
        .events
        .windows(2)
        .all(|pair| pair[0]["seq"].as_u64() < pair[1]["seq"].as_u64()));
    // A dump takes no sampler: it answers on a build that has none, and it names no stack.
    assert!(first.stacks.is_empty());
    assert_eq!((first.seconds, first.hz, first.samples), (0, 0, 0));

    // The next dump has a later sequence number.
    let second = profile_report("Profile task_dump", dump(&served).await);
    assert!(second.seq > first.seq);
    drop(held);
    drop(dumped);
}

// spec: MEDULLA-OBSERVE-R012.1
// spec: MEDULLA-OBSERVE-R012.2
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn chaos_stalled_task_and_spinning_loop_are_named() {
    use medulla_observe::registry::tasks::{TaskKind, TaskSpec};
    let served = Served::new().await;

    // StallTask: a paging task that reports once and never again is listed Stalled with the
    // location that registered it, and the graph it pages names it as the blocker. The scan
    // takes the clock as an argument, so the test waits on none.
    let graph = "observe:chaos-stall";
    let task = format!("lazy-page-in:{graph}");
    medulla_observe::readiness().open(graph);
    let stalled = medulla_observe::chaos::stall_task(&task);
    let at = stalled.row().last_progress_unix_ms;
    let found = medulla_observe::watchdog::scan(
        medulla_observe::global(),
        medulla_observe::readiness(),
        at + 501,
        500,
    );
    assert!(found.iter().any(|stalled| stalled.row.name == task));
    let page = served.page(IntrospectSection::Tasks).await;
    let row = page
        .rows
        .iter()
        .find(|row| row["name"] == task.as_str())
        .expect("the stalled task is listed");
    assert_eq!(row["state"], "stalled");
    assert!(
        row["registered_location"]
            .as_str()
            .is_some_and(|location| location.contains("observability.rs")),
        "the row names where the task was registered: {}",
        row["registered_location"]
    );
    let readiness = health(&served).await.readiness.unwrap();
    assert!(readiness
        .blockers
        .iter()
        .any(|blocker| blocker.graph == graph && blocker.task.as_deref() == Some(task.as_str())));
    medulla_observe::readiness().remove(graph);
    drop(stalled);

    // SpinLoop behind its guard ends with NO_PROGRESS, naming the loop and where the guard is.
    let error = medulla_observe::chaos::spin_guarded("chaos-spin")
        .await
        .expect_err("a loop whose cursor never moves must end");
    assert_eq!(error.name, "chaos-spin");
    assert_eq!(error.code().as_str(), "NO_PROGRESS");
    assert!(
        medulla_observe::registry::tasks::file_line(error.location).contains("observability.rs")
    );
    // The same loop as a registered task ends Failed with the typed code.
    medulla_observe::spawn::spawn(
        TaskSpec {
            name: "chaos-spin-task".to_string(),
            owner: "test".to_string(),
            kind: TaskKind::External,
        },
        async {
            medulla_observe::chaos::spin_guarded("chaos-spin-task")
                .await
                .map_err(|error| error.to_string())
        },
    )
    .await
    .unwrap();
    let row = medulla_observe::global().get("chaos-spin-task").unwrap();
    assert_eq!(row.state, medulla_observe::machine::task::TaskState::Failed);
    assert_eq!(
        row.failure
            .as_ref()
            .map(medulla_observe::machine::task::TaskFailure::code),
        Some("NO_PROGRESS")
    );

    // With the guard bypassed the loop pins one worker: the runtime section flags it spinning.
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let pinned = medulla_observe::chaos::spin_pinned(stop.clone());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let page = served.page(IntrospectSection::Runtime).await;
        if page.rows.iter().any(|row| row["spinning"] == true) {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "no worker was flagged spinning"
        );
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    // A profile taken while the worker is pinned samples the threads of the process.
    if medulla_observe::profile::available() {
        let report = profile_report(
            "Profile stacks",
            served
                .call(
                    "__commons__",
                    Method::Profile {
                        kind: medulla_types::introspect::ProfileKind::Stacks,
                        seconds: 1,
                    },
                )
                .await,
        );
        assert!(report.samples > 0);
    }
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    pinned.await.unwrap();
}

// spec: MEDULLA-OBSERVE-R012.3
#[tokio::test(flavor = "multi_thread")]
async fn frozen_paging_cursor_ends_typed_and_degrades_graph() {
    let _env_lock = medulla_kernel::crypto::acquire_test_env_lock().await;
    let served = Served::new().await;
    let graph = "observe:frozen";
    medulla_observe::chaos::freeze_cursor(graph);
    served.lazy_graph(graph, 25, 5).await;

    // The paging loop meets the same cursor again and ends NO_PROGRESS, naming itself.
    let row = finished_task(&format!("lazy-page-in:{graph}")).await;
    assert_eq!(row.state, medulla_observe::machine::task::TaskState::Failed);
    assert_eq!(
        row.failure
            .as_ref()
            .map(medulla_observe::machine::task::TaskFailure::code),
        Some("NO_PROGRESS")
    );
    // The graph is degraded, with the paging task as the reason.
    let readiness = medulla_observe::readiness()
        .get(graph)
        .expect("a readiness row");
    assert_eq!(
        readiness.state,
        medulla_observe::machine::readiness::ReadinessState::Degraded
    );
    let reason = readiness.reason.expect("a degraded graph has a reason");
    assert_eq!(reason.code, "NO_PROGRESS");
    assert_eq!(
        reason.task.as_deref(),
        Some(format!("lazy-page-in:{graph}").as_str())
    );

    // The failed partial core was evicted; the next read opens the graph again in pages of five
    // and meets the partial image: it is refused PARTIAL_MATERIALIZATION, typed on the wire.
    std::env::set_var("MEDULLA_LAZY_OPEN_PAGE_SIZE", "5");
    let read = served.call(graph, Method::NodeIds).await;
    std::env::remove_var("MEDULLA_LAZY_OPEN_PAGE_SIZE");
    medulla_observe::chaos::thaw_cursor(graph);
    assert_eq!(
        read.error.as_deref(),
        Some("PARTIAL_MATERIALIZATION"),
        "{read:?}"
    );
    assert!(read
        .error_detail
        .as_deref()
        .is_some_and(|detail| detail.contains("\"phase\"")));
}
