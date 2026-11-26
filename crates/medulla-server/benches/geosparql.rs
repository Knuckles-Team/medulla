//! Served GeoSPARQL hot-path bench (MEDULLA-FEDERATED-QUERY-R709.6).
//!
//! Loads 1,000,000 `geo:wktLiteral` points into a durable graph through served
//! SPARQL updates (the commit of each batch feeds the per-graph spatial index of
//! R709.7), then runs a `geof:sfWithin` FILTER against one polygon through a
//! signed `Method::Sparql` request: `dispatch_request_method`, `dispatch_graph_op`,
//! `run_dispatch_pipeline`, `route_rdf_gateway`, `medulla_h_sparql::rdf::try_handle`
//! and the `geof:` hook of the SPARQL expression evaluator.
//!
//! Budget: p99 at most 40 ms, read from `latency_p99_ns_max.geosparql_within` in
//! `crates/medulla-plan/benches/thresholds.json`. The stored baseline under
//! `latency_p99_ns_baseline.geosparql_within` (null until the nightly lane records
//! one) may be exceeded by at most 10 percent. A breach exits non-zero: a nightly
//! failure, never a pull request gate. The library-level kernels stay covered by
//! `crates/medulla-rdf/tests/geosparql.rs`.
//!
//! Run: `cargo bench -p medulla-server --features sparql-http,geosparql,redb,test-support --bench geosparql`.
// spec: MEDULLA-FEDERATED-QUERY-R709.6

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use medulla_protocol_types::protocol::{GraphType, Method, Response, ResultPayload, SparqlResult};
use medulla_server_api::api::dispatch_support::SPARQL_HTTP_UPDATE_EVENT;
use medulla_server_api::state::ServerState;
use medulla_server_auth::auth::{dispatch_test_on_heap, scoped_test_request_on, ScopedTestCaller};
use tokio::runtime::Runtime;
use tokio::sync::RwLock;

/// Grid side: `SIDE * SIDE` points.
const SIDE: u32 = 1_000;
/// Points per served update, so one request stays a bounded batch.
const BATCH: u32 = 10_000;
/// Grid spacing in degrees, inside the EPSG:4326 range for the whole grid.
const STEP: f64 = 0.05;
/// Cells per axis inside the query polygon; the polygon ends half a step past
/// the last of them, so no point sits on its boundary.
const INSIDE: u32 = 100;
const SAMPLES: usize = 200;
const SECRET: &str = "geosparql-bench-secret"; // sanitizer:ignore
const TENANT: &str = "tenant-shared";
const ADMIN: &str = "geosparql-bench-admin";
const GRAPH: &str = "geobench";
const GEO: &str = "http://www.opengis.net/ont/geosparql#";
const THRESHOLDS: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../medulla-plan/benches/thresholds.json"
);
const KEY: &str = "geosparql_within";
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
                "geosparql-bench:create",
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
                nonce: &format!("geosparql-bench-nonce-{id}"),
                idempotency_key: &format!("{key}:{id}"),
            },
        );
        dispatch_test_on_heap(&self.state, request).await
    }

    /// Insert the whole grid, [`BATCH`] points per served update.
    async fn load_points(&self) {
        let total = SIDE * SIDE;
        let mut start = 0;
        while start < total {
            let end = (start + BATCH).min(total);
            let mut update = String::from("INSERT DATA {\n");
            for n in start..end {
                let (x, y) = (f64::from(n % SIDE) * STEP, f64::from(n / SIDE) * STEP);
                update.push_str(&format!(
                    "  <http://ex/p{n}> <{GEO}asWKT> \"POINT({x:.2} {y:.2})\"^^<{GEO}wktLiteral> .\n"
                ));
            }
            update.push('}');
            let response = self
                .call(
                    "geosparql-bench:load",
                    Method::ApplyMutation {
                        event_type: SPARQL_HTTP_UPDATE_EVENT.to_string(),
                        query: update,
                    },
                )
                .await;
            assert!(response.error.is_none(), "{response:?}");
            start = end;
        }
    }

    /// The `sfWithin` query over the polygon that holds `INSIDE * INSIDE` points.
    async fn within(&self) -> usize {
        let edge = f64::from(INSIDE) * STEP - STEP / 2.0;
        let query = format!(
            "PREFIX geo: <{GEO}>\nPREFIX geof: <http://www.opengis.net/def/function/geosparql/>\n\
             SELECT ?f WHERE {{ ?f geo:asWKT ?w . FILTER(geof:sfWithin(?w, \
             \"POLYGON((-0.01 -0.01, {edge:.3} -0.01, {edge:.3} {edge:.3}, -0.01 {edge:.3}, -0.01 -0.01))\"^^geo:wktLiteral)) }}"
        );
        let response = self
            .call(
                "geosparql-bench:within",
                Method::Sparql {
                    query,
                    base_iri: String::new(),
                    type_convention: String::new(),
                    explain: false,
                    requires_packs: Vec::new(),
                    latency_class: Default::default(),
                    graph_media_type: None,
                },
            )
            .await;
        assert!(response.error.is_none(), "{response:?}");
        let Some(ResultPayload::Raw(bytes)) = response.result else {
            panic!("a sparql SELECT answers a raw result");
        };
        let result: SparqlResult = rmp_serde::from_slice(&bytes).expect("decode the sparql result");
        result.rows.len()
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
    runtime.block_on(served.load_points());
    let expected = (INSIDE * INSIDE) as usize;
    let mut samples = Vec::with_capacity(SAMPLES);
    for _ in 0..SAMPLES {
        let started = Instant::now();
        let rows = runtime.block_on(served.within());
        samples.push(started.elapsed());
        assert_eq!(rows, expected, "the polygon holds exactly its grid cells");
    }
    samples.sort();
    let p99 = samples[(samples.len() * 99 / 100).min(samples.len() - 1)];
    let budget = Duration::from_nanos(
        threshold("latency_p99_ns_max").expect("budget geosparql_within in thresholds.json") as u64,
    );
    println!("served/geosparql_within p99 = {p99:?} (budget {budget:?})");
    assert!(p99 <= budget, "p99 {p99:?} exceeds {budget:?}");
    if let Some(baseline) = threshold("latency_p99_ns_baseline") {
        let ceiling = Duration::from_nanos((baseline * (1.0 + REGRESSION_TOLERANCE)) as u64);
        assert!(
            p99 <= ceiling,
            "p99 {p99:?} regressed more than 10 percent past {baseline} ns"
        );
    }
}
