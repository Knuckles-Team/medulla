//! Source permission sync at batch size (MEDULLA-INGEST-WORKER-R902.6).
//!
//! Nightly bench group `ingest/source_permission_sync`: `prepare`, the served
//! `SourceIngest` admission entry point, runs 100 `access` batches of 1,000
//! snapshots each against records the engine already holds in its durable store.
//! Every batch reads each held record, applies the admission rules and compiles the
//! compare-and-set updates into one change envelope. Budget: p99 per batch below
//! 20 ms. The group expansion a read adds on top is benched beside it in
//! `ingest_group_expansion` (MEDULLA-IDENTITY-R706, under 50 microseconds at p99).
//! The run exits non-zero when a budget is exceeded. A service level, not a pull
//! request gate.
//!
// spec: MEDULLA-INGEST-WORKER-R902.6
//!
//! Run: cargo bench --bench ingest_permission_sync

use std::sync::Arc;
use std::time::{Duration, Instant};

use medulla_h_ingest::source_ingestion::{prepare, source_node_id, PrepareContext};
use medulla_protocol_types::protocol::{GraphType, Method};
use medulla_server_api::api::persistence::PersistenceBackend;
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_server_api::state::ServerState;
use medulla_source_types::source_ingestion::SourceIngestionRequest;
use tokio::sync::RwLock;

const BATCHES: usize = 100;
const SNAPSHOTS: usize = 1_000;
const BUDGET_P99: Duration = Duration::from_millis(20);
const GRAPH: &str = "bench-source-permissions";
const TENANT: &str = "tenant-shared";
const CONNECTOR: &str = "wiki";
const STREAM: &str = "pages";

fn record_ref(index: usize) -> String {
    format!("page-{index}")
}

fn access_batch(page: usize) -> SourceIngestionRequest {
    let snapshots: Vec<serde_json::Value> = (0..SNAPSHOTS)
        .map(|index| {
            serde_json::json!({
                "source": CONNECTOR,
                "record_ref": record_ref(index),
                "access_public": index % 5 == 0,
                "read_principals": [format!("usr:{index}"), "usr:auditor"],
                "read_groups": ["eng", "ops"],
                "synced_at": 1_700_000_000_000u64 + page as u64,
            })
        })
        .collect();
    serde_json::from_value(serde_json::json!({
        "connector": CONNECTOR,
        "kind": "access",
        "mode": "delta",
        "strict_schema": true,
        "records": [],
        "relationships": [],
        "withdrawals": [],
        "extension": { "access": snapshots },
        "provider_checkpoint": { "stream": STREAM, "position": { "permissions": page } },
        "expected_previous_checkpoint": null,
    }))
    .expect("a valid access batch")
}

async fn seed_held_records(persistence: &Arc<dyn PersistenceBackend>, graph_fname: &str) {
    let mut writes = tokio::task::JoinSet::new();
    for index in 0..SNAPSHOTS {
        let persistence = persistence.clone();
        let graph_fname = graph_fname.to_string();
        let node_id =
            source_node_id(TENANT, CONNECTOR, STREAM, &record_ref(index)).expect("record node id");
        writes.spawn(async move {
            let properties = serde_json::json!({
                "type": "Document",
                "name": format!("page {index}"),
                "source_access_public": true,
                "source_access_read_roles": [],
                "source_access_markings": ["internal"],
            });
            persistence
                .record_durable(
                    &graph_fname,
                    &Method::AddNode {
                        node_id,
                        properties_msgpack: rmp_serde::to_vec_named(&properties)
                            .expect("properties encode"),
                    },
                )
                .await
        });
    }
    while let Some(done) = writes.join_next().await {
        done.expect("seed task").expect("held record is durable");
    }
}

fn main() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let dir = tempfile::tempdir().expect("persist dir");
    let backend = medulla_persistence::redb_backend::RedbBackend::open(
        dir.path().to_string_lossy().into(),
        64,
    )
    .expect("open the durable store");
    let persistence: Arc<dyn PersistenceBackend> = Arc::new(backend);
    let graph_fname = medulla_kernel::persist::sanitize(GRAPH);
    runtime.block_on(async {
        persistence
            .register_graph(&graph_fname, GRAPH, GraphType::Commons)
            .await
            .expect("register the graph");
        seed_held_records(&persistence, &graph_fname).await;
    });

    let verified = VerifiedRequestContext::verified_for_test_with_scopes(
        "bench-permission-sync",
        TENANT,
        &["source:ingest", "kg:read", "kg:write"],
    );
    let state = Arc::new(RwLock::new(ServerState::new_for_test(
        "bench-permission-sync",
        ServerState::test_isolation("bench-permission-sync"),
    )));
    runtime.block_on(async {
        state.write().await.persistence = Some(persistence.clone());
    });

    let requests: Vec<SourceIngestionRequest> = (0..BATCHES).map(access_batch).collect();
    let mut samples = Vec::with_capacity(BATCHES);
    for request in requests {
        let started = Instant::now();
        let prepared = runtime.block_on(prepare(
            PrepareContext {
                state: &state,
                request_id: 1,
                graph_name: GRAPH,
                tenant_scope: TENANT,
                verified: &verified,
                graph_version: 0,
                placement_epoch: 0,
                fencing_token: None,
            },
            request,
        ));
        samples.push(started.elapsed());
        let prepared = prepared.expect("an access batch is admitted");
        assert_eq!(
            prepared.affected_count(),
            SNAPSHOTS as u64,
            "every snapshot names a held record, so each one updates"
        );
        std::hint::black_box(prepared);
    }
    samples.sort();
    let p99 = samples[(samples.len() * 99 / 100).min(samples.len() - 1)];
    println!(
        "ingest/source_permission_sync p99 per batch of {SNAPSHOTS} = {p99:?} (budget {BUDGET_P99:?})"
    );
    assert!(p99 < BUDGET_P99, "p99 {p99:?} exceeds {BUDGET_P99:?}");
}
