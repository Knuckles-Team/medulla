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
        self.call_as(graph, method, vec!["*".to_string()]).await
    }

    /// One request as `PRINCIPAL` holding exactly `scopes`.
    async fn call_as(&self, graph: &str, method: Method, scopes: Vec<String>) -> Response {
        let id = next_id();
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
            trace_id: None,
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
    assert!(served.state.write().await.registry.evict_resident("observe:ready"));
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
    assert!(!principal_ref.contains(PRINCIPAL), "the principal is never listed");
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
    assert!(!readiness.ready, "a blocked graph makes the engine not ready");
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
    assert!(readiness.blockers.iter().all(|blocker| blocker.graph != opening));
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
    assert!(report
        .blockers
        .iter()
        .any(|blocker| blocker.graph == graph
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
        ("Failed".to_string(), Some("MATVIEW_RELOAD_FAILED".to_string()))
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
        assert!(!text.contains(forbidden), "an Introspect row holds `{forbidden}`");
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
    let denial = denied.error.expect("a caller without observe:read is refused");
    assert!(!denial.contains(graph), "the refusal names a graph: {denial}");
    // The call writes nothing: the catalog is unchanged.
    assert_eq!(served.state.read().await.registry.catalog_len(), catalog_before);
}
