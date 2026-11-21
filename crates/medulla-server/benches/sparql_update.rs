//! Served SPARQL update hot-path bench (MEDULLA-FEDERATED-QUERY-R707.6).
//!
//! Sends 100 signed `Method::ApplyMutation { event_type: SPARQL_HTTP_UPDATE_EVENT, query }`
//! requests to a durable graph, each an `INSERT DATA` of 10,000 triples with fresh subjects, and
//! times every call: `dispatch_request_method`, `dispatch_sparql_http_update` and
//! `coordinated_sparql_http_update`. `medulla-rdf` sits below the server crates and cannot
//! dispatch a served method, so the bench lives beside the served GeoSPARQL bench.
//!
//! Budget: p99 at most `latency_p99_ns_max.sparql_update_insert_10k` in
//! `crates/medulla-plan/benches/thresholds.json`. The stored baseline under
//! `latency_p99_ns_baseline.sparql_update_insert_10k` (null until the nightly lane records
//! one) may be exceeded by at most 10 percent. A breach exits non-zero: a nightly failure,
//! never a pull request gate.
//!
//! Run: `cargo bench -p medulla-server --features sparql-http,redb,test-support --bench sparql_update`.
// spec: MEDULLA-FEDERATED-QUERY-R707.6

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use medulla_protocol_types::protocol::{GraphType, Method, Response};
use medulla_server_api::api::dispatch_support::SPARQL_HTTP_UPDATE_EVENT;
use medulla_server_api::state::ServerState;
use medulla_server_auth::auth::{dispatch_test_on_heap, scoped_test_request_on, ScopedTestCaller};
use tokio::runtime::Runtime;
use tokio::sync::RwLock;

/// Triples per served update.
const TRIPLES: u32 = 10_000;
const SAMPLES: usize = 100;
const SECRET: &str = "sparql-update-bench-secret"; // sanitizer:ignore
const TENANT: &str = "tenant-shared";
const ADMIN: &str = "sparql-update-bench-admin";
const GRAPH: &str = "sparqlupdatebench";
const THRESHOLDS: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../medulla-plan/benches/thresholds.json"
);
const KEY: &str = "sparql_update_insert_10k";
const REGRESSION_TOLERANCE: f64 = 0.1;

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

struct Served {
    _dir: tempfile::TempDir,
    state: Arc<RwLock<ServerState>>,
}

impl Served {
    async fn open() -> Self {
        let dir = tempfile::tempdir().expect("scratch directory");
        let mut state = medulla_server::seams_install::new_state_for_test(
            SECRET,
            ServerState::test_isolation(ADMIN),
        );
        state.persist_dir = Some(dir.path().to_string_lossy().into_owned());
        let root = dir.path().join("graphs");
        std::fs::create_dir_all(&root).expect("graph directory");
        let backend = medulla_persistence::redb_backend::RedbBackend::open(
            root.to_string_lossy().into_owned(),
            64,
        )
        .expect("open the graph backend");
        state.persistence = Some(Arc::new(backend));
        let served = Self {
            _dir: dir,
            state: Arc::new(RwLock::new(state)),
        };
        let created = served
            .call(
                "sparql-update-bench:create",
                Method::CreateGraph {
                    graph_name: GRAPH.to_string(),
                    graph_type: GraphType::Global,
                },
            )
            .await;
        assert!(created.error.is_none(), "{created:?}");
        served
    }

    /// One signed request on the bench graph, unique in id, nonce and key.
    async fn call(&self, key: &str, method: Method) -> Response {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let request = scoped_test_request_on(
            SECRET,
            id,
            GRAPH,
            method,
            ScopedTestCaller {
                principal: ADMIN,
                tenant: TENANT,
                scopes: &["*"],
                nonce: &format!("sparql-update-bench-nonce-{id}"),
                idempotency_key: &format!("{key}:{id}"),
            },
        );
        dispatch_test_on_heap(&self.state, request).await
    }
}

impl Served {
    /// One `INSERT DATA` of [`TRIPLES`] triples with subjects no other sample uses.
    async fn insert(&self, sample: usize) -> Response {
        let mut update = String::from("INSERT DATA {\n");
        for i in 0..TRIPLES {
            update.push_str(&format!(
                "  <http://ex/s{sample}/n{i}> <http://ex/p> \"{i}\" .\n"
            ));
        }
        update.push('}');
        self.call(
            "sparql-update-bench:insert",
            Method::ApplyMutation {
                event_type: SPARQL_HTTP_UPDATE_EVENT.to_string(),
                query: update,
            },
        )
        .await
    }
}

fn threshold(section: &str) -> Option<f64> {
    let text = std::fs::read_to_string(THRESHOLDS).expect("read thresholds.json");
    let value: serde_json::Value = serde_json::from_str(&text).expect("parse thresholds.json");
    value.get(section)?.get(KEY)?.as_f64()
}

fn main() {
    let runtime = Runtime::new().expect("runtime");
    let _env = runtime.block_on(medulla_kernel::crypto::acquire_test_env_read_lock());
    let served = runtime.block_on(Served::open());
    let mut samples = Vec::with_capacity(SAMPLES);
    for sample in 0..SAMPLES {
        let started = Instant::now();
        let response = runtime.block_on(served.insert(sample));
        samples.push(started.elapsed());
        assert!(response.error.is_none(), "{response:?}");
    }
    samples.sort();
    // Nearest-rank p99.
    let rank = (samples.len() * 99).div_ceil(100).max(1);
    let p99 = samples[rank - 1];
    let budget = Duration::from_nanos(
        threshold("latency_p99_ns_max").expect("budget sparql_update_insert_10k in thresholds.json")
            as u64,
    );
    println!("served/{KEY} p99 = {p99:?} (budget {budget:?})");
    assert!(p99 <= budget, "p99 {p99:?} exceeds {budget:?}");
    if let Some(baseline) = threshold("latency_p99_ns_baseline") {
        let ceiling = Duration::from_nanos((baseline * (1.0 + REGRESSION_TOLERANCE)) as u64);
        assert!(
            p99 <= ceiling,
            "p99 {p99:?} regressed more than 10 percent past {baseline} ns"
        );
    }
}
