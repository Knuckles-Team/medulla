//! Cross-modal transaction tests that drive the persistence backend through the
//! transaction handler (`h-txn`), which sits above the persistence crate.

use std::sync::Arc;

use tokio::sync::RwLock;

use medulla_persistence::redb_backend::RedbBackend;
use medulla_persistence::AsRedb;
use medulla_server_api::api::persistence::PersistenceBackend;
use medulla_server_auth::auth::dispatch_test_on_heap as dispatch_on_heap;
use medulla_server_api::state::ServerState;
use medulla_store::redb_layout::shard_filename;
use medulla_store::redb_store::shard::Shard;
use medulla_protocol_types::protocol::*;

const TEST_AGENT: &str = "unit-test-agent";

#[cfg(feature = "tsdb")]
fn scoped_series_key(graph: &str, series: &str, principal: &str) -> String {
    use sha2::Digest;
    let tenant_scope = medulla_server_api::batch_compile::opaque_coordinator_key(
        "carrier-tenant",
        "verified",
        "tenant-shared",
    );
    let actor_scope = format!(
        "principal:sha256:{}",
        hex::encode(sha2::Sha256::digest(principal.as_bytes()))
    );
    let owner_scope = medulla_server_api::batch_compile::opaque_coordinator_key(
        "carrier-owner",
        &tenant_scope,
        &actor_scope,
    );
    let graph_scope = medulla_server_api::batch_compile::opaque_coordinator_key(
        "timeseries-graph",
        &owner_scope,
        graph,
    );
    medulla_tsdb::store::SeriesKey::new(tenant_scope, graph_scope, series).encode()
}

#[cfg(feature = "tsdb")]
fn direct_test_series_key(graph: &str, series: &str) -> String {
    scoped_series_key(graph, series, &format!("principal:{TEST_AGENT}"))
}

#[cfg(feature = "tsdb")]
fn envelope_test_series_key(graph: &str, series: &str) -> String {
    scoped_series_key(graph, series, TEST_AGENT)
}

fn props(v: serde_json::Value) -> Vec<u8> {
    rmp_serde::to_vec_named(&v).unwrap()
}

fn new_state(persist_dir: Option<String>) -> Arc<RwLock<ServerState>> {
    let mut state = medulla_server::seams_install::new_state_for_test("test", ServerState::test_isolation(TEST_AGENT));
    state.persist_dir = persist_dir;
    Arc::new(RwLock::new(state))
}

fn current_request(secret: &str, id: u64, graph: &str, method: Method) -> Request {
    medulla_server_auth::auth::build_shared_test_request(secret, id, graph, TEST_AGENT, method)
}

use medulla_h_txn::txn::try_handle as txn_try_handle;
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_protocol_types::protocol::Response;
use medulla_protocol_types::protocol::ResultPayload;

fn cm_dir(tag: &str) -> String {
    // The cross-modal handler-commit path seals its transaction recovery plan
    // (`medulla_h_txn::txn::seal_txn_recovery_plan`), which fail-closed REQUIRES
    // `MEDULLA_ENCRYPTION_KEY` to be configured — the same seal requirement
    // the xshard harness hit. Provision it ONCE before any backend opens. Encryption
    // is symmetric and transparent to every durable round-trip these tests make, so
    // a keyed store behaves identically for their assertions. The env var is
    // process-global, and it is now ONE shared value for the whole binary (see
    // `medulla_kernel::crypto::TEST_AT_REST_KEY`'s doc -- a per-module value is what made
    // the ambient key change up to seven times over one run). Every caller of
    // `cm_dir` holds `medulla_kernel::crypto::acquire_test_env_lock()` for its entire test
    // body (see each call site), so the provisioning below always runs under that
    // lock -- do NOT acquire it here, `tokio::sync::RwLock` is not reentrant.
    medulla_kernel::crypto::provision_test_at_rest_key_under_write_guard();
    let d = std::env::temp_dir().join(format!("medulla-crossmodal-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d.to_string_lossy().to_string()
}

/// A stage op answers `Bool`; `Commit` declares its unkeyed outcome as the JSON
/// boolean, the same bytes on the wire.
fn as_bool(r: Response) -> Option<bool> {
    match r.result {
        Some(ResultPayload::Bool(b)) | Some(ResultPayload::Json(serde_json::Value::Bool(b))) => {
            Some(b)
        }
        _ => None,
    }
}

async fn txn_handle(
    state: &Arc<RwLock<ServerState>>,
    req_id: u64,
    _caller: Option<&str>,
    method: Method,
) -> Result<Response, Method> {
    let context = VerifiedRequestContext::verified_for_test(TEST_AGENT);
    txn_try_handle(state, req_id, TEST_AGENT, &context, method).await
}

/// Drive BeginTxn(g) → TxnAddNode(node) → TxnAddEmbedding → TxnBlobRef, returning
/// the txn id (the staged cross-modal write-set is ready to Commit).
async fn stage_crossmodal(
    state: &Arc<RwLock<ServerState>>,
    graph: &str,
    node: &str,
    digest: &str,
) -> String {
    let begin = txn_handle(
        state,
        1,
        None,
        Method::BeginTxn {
            graph: Some(graph.to_string()),
            isolation: None,
        },
    )
    .await
    .unwrap();
    let txn_id = match begin.result {
        Some(ResultPayload::String(id)) => id,
        other => panic!("BeginTxn id, got {other:?}"),
    };
    assert_eq!(
        as_bool(
            txn_handle(
                state,
                2,
                None,
                Method::TxnAddNode {
                    txn_id: txn_id.clone(),
                    node_id: node.to_string(),
                    properties_msgpack: props(serde_json::json!({"type": "Media"})),
                    graph: None,
                },
            )
            .await
            .unwrap()
        ),
        Some(true)
    );
    assert_eq!(
        as_bool(
            txn_handle(
                state,
                3,
                None,
                Method::TxnAddEmbedding {
                    txn_id: txn_id.clone(),
                    node_id: node.to_string(),
                    embedding: vec![0.1, 0.2, 0.3],
                    graph: None,
                },
            )
            .await
            .unwrap()
        ),
        Some(true)
    );
    assert_eq!(
        as_bool(
            txn_handle(
                state,
                4,
                None,
                Method::TxnBlobRef {
                    txn_id: txn_id.clone(),
                    node_id: node.to_string(),
                    digest: digest.to_string(),
                    graph: None,
                },
            )
            .await
            .unwrap()
        ),
        Some(true)
    );
    txn_id
}

/// HAPPY: a cross-modal txn (node + vector + blob-ref) commits atomically — ALL
/// modalities land durably in ONE WriteTransaction and survive a reload.
#[tokio::test(flavor = "multi_thread")]
async fn crossmodal_txn_commits_all_modalities_atomically() {
    // Held for the whole test: `cm_dir` provisions `MEDULLA_ENCRYPTION_KEY`
    // once (process-global) and this test's backend opens depend on it staying set
    // throughout — see `medulla_kernel::crypto::acquire_test_env_lock`'s doc.
    let _env_lock = medulla_kernel::crypto::acquire_test_env_lock().await;
    let dir = cm_dir("happy");
    let backend: Arc<dyn PersistenceBackend> =
        Arc::new(RedbBackend::open(dir.clone(), 64).unwrap());
    let state = new_state(Some(dir.clone()));
    {
        let mut s = state.write().await;
        let _ = s.registry.create_graph("media", GraphType::Global, None);
        s.persistence = Some(backend.clone());
    }

    let txn_id = stage_crossmodal(&state, "media", "m1", "sha256:abc").await;
    // Nothing applied before commit.
    {
        let s = state.read().await;
        let core = s.registry.get("media").unwrap().core.clone();
        assert!(!core.has_node("m1"), "no apply before commit");
        assert_eq!(
            core.semantic_store.read().len(),
            0,
            "no vector before commit"
        );
    }

    // COMMIT — all three modalities land atomically.
    assert_eq!(
        as_bool(
            txn_handle(
                &state,
                5,
                None,
                Method::Commit {
                    txn_id,
                    idempotency_key: None
                }
            )
            .await
            .unwrap()
        ),
        Some(true),
        "cross-modal commit"
    );

    // In-memory: node + vector + blob-ref property all present.
    {
        let s = state.read().await;
        let core = s.registry.get("media").unwrap().core.clone();
        assert!(core.has_node("m1"), "node landed");
        assert_eq!(core.semantic_store.read().len(), 1, "vector landed");
        let blob = core.get_node_properties("m1").unwrap();
        let p: serde_json::Map<String, serde_json::Value> = rmp_serde::from_slice(&blob).unwrap();
        assert_eq!(
            p.get("__blob__").and_then(|v| v.as_str()),
            Some("sha256:abc")
        );
    }
    backend.shutdown();
    drop(backend);
    // `state` holds a clone of the backend Arc; drop it so the LAST handle is gone
    // and the exclusive per-process redb file lock is released before the reopen.
    drop(state);

    // Reload from redb: every modality is DURABLE (the one WriteTransaction).
    let backend2: Arc<dyn PersistenceBackend> =
        Arc::new(RedbBackend::open(dir.clone(), 64).unwrap());
    let state2 = new_state(Some(dir.clone()));
    backend2.load_all(&state2).await.unwrap();
    {
        let s = state2.read().await;
        let core = s.registry.get("media").unwrap().core.clone();
        assert!(core.has_node("m1"), "node durable");
        assert_eq!(core.semantic_store.read().len(), 1, "vector durable");
        let blob = core.get_node_properties("m1").unwrap();
        let p: serde_json::Map<String, serde_json::Value> = rmp_serde::from_slice(&blob).unwrap();
        assert_eq!(
            p.get("__blob__").and_then(|v| v.as_str()),
            Some("sha256:abc"),
            "blob-ref durable"
        );
    }
    backend2.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

/// A backend whose cross-modal DURABLE COMMIT always FAILS — used to prove the
/// handler rolls back ALL modalities (applies nothing in-memory) on a
/// durable-commit failure: no partial cross-modal commit
/// (CONCEPT:MEDULLA-KG.txn.reader-never-sees-node).
///
/// Everything that is NOT the injected failure delegates to a real
/// `RedbBackend`, including the `as_redb` downcast. That delegation is load
/// bearing twice over, and both halves were missing:
///
/// * `as_redb` defaults to `None` on the trait (`persistence/mod.rs`), and
///   `medulla_h_txn::txn::begin_txn_lifecycle_receipt` refuses every
///   txn-lifecycle method — `BeginTxn` included — without it ("transaction
///   lifecycle requires durable redb"). So this double failed the very FIRST
///   call of `stage_crossmodal`, and both tests using it died on
///   `panic!("BeginTxn id, got None")` before reaching the rollback they
///   name. The staging half of the scenario was never exercised at all.
/// * the commit path calls `commit_mutation_batch_crossmodal`, not the
///   lower-level `commit_crossmodal` this double used to override. Overriding
///   only the latter left the injection attached to a method the handler no
///   longer calls: the commit would have failed anyway, but on the trait's
///   own "backend does not support atomic cross-modal MutationBatch commits"
///   default — i.e. the test would have asserted rollback after a
///   NOT-IMPLEMENTED error rather than after the mid-way durable failure it
///   describes. Both are overridden now, with the same injected message, so
///   whichever seam a future refactor routes through, the failure the test
///   names is the failure it gets.
struct FailingBackend {
    inner: Arc<RedbBackend>,
}

#[async_trait::async_trait]
impl PersistenceBackend for FailingBackend {
    async fn load_all(&self, s: &Arc<RwLock<ServerState>>) -> Result<usize, String> {
        self.inner.load_all(s).await
    }
    async fn record_durable(&self, g: &str, m: &Method) -> Result<(), String> {
        self.inner.record_durable(g, m).await
    }
    fn as_any(&self) -> Option<&dyn std::any::Any> {
        self.inner.as_any()
    }
    async fn commit_crossmodal(
        &self,
        _g: &str,
        _m: &[Method],
        _v: &[(String, Vec<f32>)],
        _b: &[(String, String)],
        _meas: &[medulla_store::MeasurementBatch],
    ) -> Result<(), String> {
        // Simulate a mid-way durable failure: NOTHING is written to redb.
        Err("injected durable commit failure".to_string())
    }
    async fn commit_mutation_batch_crossmodal(
        &self,
        _args: medulla_server_api::api::persistence::CrossModalCommitArgs<'_>,
    ) -> Result<medulla_protocol_types::mutation_batch::MutationBatchCommit, String> {
        // The seam the cross-modal Commit handler actually calls.
        Err("injected durable commit failure".to_string())
    }
    fn shutdown(&self) {
        self.inner.shutdown()
    }
}

/// ROLLBACK: a cross-modal txn whose durable commit FAILS mid-way applies NONE of
/// its modalities — no node, no vector, no blob-ref (no partial commit).
#[tokio::test(flavor = "multi_thread")]
async fn crossmodal_txn_rolls_back_all_modalities_on_failure() {
    // See `crossmodal_txn_commits_all_modalities_atomically` above: held for the
    // whole test.
    let _env_lock = medulla_kernel::crypto::acquire_test_env_lock().await;
    let dir = cm_dir("rollback");
    let inner = Arc::new(RedbBackend::open(dir.clone(), 64).unwrap());
    let backend: Arc<dyn PersistenceBackend> = Arc::new(FailingBackend {
        inner: inner.clone(),
    });
    let state = new_state(Some(dir.clone()));
    {
        let mut s = state.write().await;
        let _ = s.registry.create_graph("media", GraphType::Global, None);
        s.persistence = Some(backend.clone());
    }

    let txn_id = stage_crossmodal(&state, "media", "m1", "sha256:def").await;

    // COMMIT must FAIL (the durable barrier errored) → Response is an error.
    let resp = txn_handle(
        &state,
        5,
        None,
        Method::Commit {
            txn_id,
            idempotency_key: None,
        },
    )
    .await
    .unwrap();
    assert!(resp.error.is_some(), "commit surfaces the durable failure");
    assert!(resp.result.is_none(), "no Bool ack on a failed commit");

    // NO PARTIAL COMMIT: NONE of the modalities applied in-memory.
    {
        let s = state.read().await;
        let core = s.registry.get("media").unwrap().core.clone();
        assert!(!core.has_node("m1"), "node rolled back");
        assert_eq!(core.semantic_store.read().len(), 0, "vector rolled back");
    }

    // And NONE durable in redb either (the WriteTransaction never committed).
    inner.shutdown();
    drop(inner);
    drop(backend);
    // `state` holds a clone of the wrapper backend (which owns `inner`); drop it so
    // the last handle is gone and the redb file lock releases before the reopen.
    drop(state);
    let backend2: Arc<dyn PersistenceBackend> =
        Arc::new(RedbBackend::open(dir.clone(), 64).unwrap());
    let state2 = new_state(Some(dir.clone()));
    backend2.load_all(&state2).await.unwrap();
    {
        let s = state2.read().await;
        let durable_node = s
            .registry
            .get("media")
            .map(|e| e.core.has_node("m1"))
            .unwrap_or(false);
        assert!(!durable_node, "node never landed durably");
    }
    backend2.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

// ── Extended cross-modal ACID: 5 modalities in ONE wtx (CONCEPT:MEDULLA-KG.backend.cross-modal-atomic-commit/361/362) ──

/// A data-independent CONSTRUCT that yields exactly one triple
/// `<urn:a> <urn:p> <urn:b>` via an inline VALUES row (needs no committed data), so
/// its staged lowering is a deterministic node+node+edge write.
#[cfg(all(feature = "tsdb", feature = "sparql"))]
const CONSTRUCT_Q: &str =
    "CONSTRUCT { ?x <urn:p> ?y } WHERE { VALUES (?x ?y) { (<urn:a> <urn:b>) } }";

/// Encode a `Vec<(i64, Vec<f64>)>` measurement batch to the wire blob.
#[cfg(all(feature = "tsdb", feature = "sparql"))]
fn meas_points(pts: &[(i64, Vec<f64>)]) -> Vec<u8> {
    rmp_serde::to_vec(&pts.to_vec()).unwrap()
}

/// Stage all FIVE modalities into a fresh txn: graph node + embedding + blob-ref +
/// time-series measurement + SPARQL CONSTRUCT triple. Returns the txn id.
#[cfg(all(feature = "tsdb", feature = "sparql"))]
async fn stage_five_modalities(
    state: &Arc<RwLock<ServerState>>,
    graph: &str,
    node: &str,
    digest: &str,
    series: &str,
    points: &[(i64, Vec<f64>)],
) -> String {
    let begin = txn_handle(
        state,
        1,
        None,
        Method::BeginTxn {
            graph: Some(graph.to_string()),
            isolation: None,
        },
    )
    .await
    .unwrap();
    let txn_id = match begin.result {
        Some(ResultPayload::String(id)) => id,
        other => panic!("BeginTxn id, got {other:?}"),
    };
    let ok = |r: Response| assert_eq!(as_bool(r), Some(true));
    ok(txn_handle(
        state,
        2,
        None,
        Method::TxnAddNode {
            txn_id: txn_id.clone(),
            node_id: node.to_string(),
            properties_msgpack: props(serde_json::json!({"type": "Media"})),
            graph: None,
        },
    )
    .await
    .unwrap());
    ok(txn_handle(
        state,
        3,
        None,
        Method::TxnAddEmbedding {
            txn_id: txn_id.clone(),
            node_id: node.to_string(),
            embedding: vec![0.1, 0.2, 0.3],
            graph: None,
        },
    )
    .await
    .unwrap());
    ok(txn_handle(
        state,
        4,
        None,
        Method::TxnBlobRef {
            txn_id: txn_id.clone(),
            node_id: node.to_string(),
            digest: digest.to_string(),
            graph: None,
        },
    )
    .await
    .unwrap());
    ok(txn_handle(
        state,
        5,
        None,
        Method::TxnAddMeasurement {
            txn_id: txn_id.clone(),
            series: series.to_string(),
            points: meas_points(points),
            graph: None,
        },
    )
    .await
    .unwrap());
    ok(txn_handle(
        state,
        6,
        None,
        Method::TxnConstruct {
            txn_id: txn_id.clone(),
            sparql: CONSTRUCT_Q.to_string(),
            graph: None,
        },
    )
    .await
    .unwrap());
    txn_id
}

/// CAPSTONE: one txn stages node + embedding + blob-ref + measurement + CONSTRUCT
/// triple; `Commit` lands ALL FIVE atomically in ONE redb `WriteTransaction`; every
/// modality is durably present after a full backend reload (CONCEPT:MEDULLA-KG.backend.cross-modal-atomic-commit/361/362).
#[cfg(all(feature = "tsdb", feature = "sparql"))]
#[tokio::test(flavor = "multi_thread")]
async fn five_modality_atomic_commit() {
    use medulla_tsdb::store::SeriesStore;

    // See `crossmodal_txn_commits_all_modalities_atomically` above: held for the
    // whole test.
    let _env_lock = medulla_kernel::crypto::acquire_test_env_lock().await;
    let dir = cm_dir("five");
    let points = vec![
        (1_000_000_000i64, vec![10.0]),
        (2_000_000_000i64, vec![20.0]),
    ];
    let backend: Arc<dyn PersistenceBackend> =
        Arc::new(RedbBackend::open(dir.clone(), 64).unwrap());
    // The measurement modality of this cross-modal commit is written to the tsdb
    // SeriesStore, so the state MUST carry one (same setup the measurement +
    // reconciliation tests use) — without it the Commit fails writing the tsdb leg.
    let series_store = Arc::new(
        SeriesStore::open_in_dir(
            std::path::Path::new(&dir),
            medulla_kernel::store_authority::process_verifier(),
            medulla_kernel::store_authority::process_authority().principal(),
            &medulla_kernel::store_authority::process_authority().proof(),
        )
        .unwrap(),
    );
    let state = new_state(Some(dir.clone()));
    {
        let mut s = state.write().await;
        let _ = s.registry.create_graph("media", GraphType::Global, None);
        s.persistence = Some(backend.clone());
        s.tsdb_store = Some(series_store.clone());
    }

    let txn_id =
        stage_five_modalities(&state, "media", "m1", "sha256:abc", "sensor", &points).await;
    // Nothing applied before commit.
    {
        let s = state.read().await;
        let core = s.registry.get("media").unwrap().core.clone();
        assert!(!core.has_node("m1"), "no apply before commit");
        assert!(!core.has_node("<urn:a>"), "no CONSTRUCT node before commit");
        assert_eq!(
            core.semantic_store.read().len(),
            0,
            "no vector before commit"
        );
    }

    assert_eq!(
        as_bool(
            txn_handle(
                &state,
                7,
                None,
                Method::Commit {
                    txn_id,
                    idempotency_key: None
                }
            )
            .await
            .unwrap()
        ),
        Some(true),
        "five-modality commit"
    );

    // In-memory: graph modalities all present (node + vector + blob + CONSTRUCT edge).
    {
        let s = state.read().await;
        let core = s.registry.get("media").unwrap().core.clone();
        assert!(core.has_node("m1"), "node landed");
        assert_eq!(core.semantic_store.read().len(), 1, "vector landed");
        assert!(
            core.has_node("<urn:a>") && core.has_node("<urn:b>"),
            "CONSTRUCT nodes landed"
        );
        let blob = core.get_node_properties("m1").unwrap();
        let p: serde_json::Map<String, serde_json::Value> = rmp_serde::from_slice(&blob).unwrap();
        assert_eq!(
            p.get("__blob__").and_then(|v| v.as_str()),
            Some("sha256:abc")
        );
    }
    backend.shutdown();
    drop(backend);
    // `state` holds a clone of the backend Arc; drop it so the last handle is gone
    // and the redb file lock releases before the same-dir reopens below.
    drop(state);

    // Measurements are durable in the authoritative shard's SERIES tables
    // (same wtx, not series.redb).
    {
        // graph-0.redb is a GraphShard owner file; reopen it through its
        // sole owner and read the file-wide SERIES tables from the control
        // scope. Opening it as a standalone SeriesStore would ask the
        // storage kernel for a different owner manifest.
        let shard =
            Shard::open(std::path::Path::new(&dir).join(shard_filename(0)).as_path()).unwrap();
        let read = shard.control_read().unwrap();
        let key = direct_test_series_key("media", "sensor");
        let meta = medulla_tsdb::store::meta_in_rtx(&read, &key)
            .unwrap()
            .expect("series durable");
        assert_eq!(meta.count, 2, "both measurement points durable");
        let scanned = medulla_tsdb::store::range_in_rtx(
            &read,
            &key,
            medulla_tsdb::point::Ts::MIN,
            medulla_tsdb::point::Ts::MAX,
        )
        .unwrap();
        assert_eq!(scanned.len(), 2, "measurement points readable post-reload");
        assert_eq!(scanned[0].values, vec![10.0]);
        assert_eq!(scanned[1].values, vec![20.0]);
    }

    // Reload the graph tier: node + vector + blob-ref + CONSTRUCT triple all durable.
    let backend2: Arc<dyn PersistenceBackend> =
        Arc::new(RedbBackend::open(dir.clone(), 64).unwrap());
    let state2 = new_state(Some(dir.clone()));
    backend2.load_all(&state2).await.unwrap();
    {
        let s = state2.read().await;
        let core = s.registry.get("media").unwrap().core.clone();
        assert!(core.has_node("m1"), "node durable");
        assert_eq!(core.semantic_store.read().len(), 1, "vector durable");
        assert!(
            core.has_node("<urn:a>") && core.has_node("<urn:b>"),
            "CONSTRUCT nodes durable"
        );
        let blob = core.get_node_properties("m1").unwrap();
        let p: serde_json::Map<String, serde_json::Value> = rmp_serde::from_slice(&blob).unwrap();
        assert_eq!(
            p.get("__blob__").and_then(|v| v.as_str()),
            Some("sha256:abc"),
            "blob durable"
        );
    }
    backend2.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

/// ATOMICITY: a five-modality txn whose durable commit FAILS mid-way (the always-fails
/// backend, injected AFTER the measurement is staged) lands NONE of the five — no node,
/// no vector, no CONSTRUCT triple in-memory, and NO series durable (CONCEPT:MEDULLA-KG.backend.cross-modal-atomic-commit).
#[cfg(all(feature = "tsdb", feature = "sparql"))]
#[tokio::test(flavor = "multi_thread")]
async fn five_modality_rolls_back_all_on_failure() {
    // See `crossmodal_txn_commits_all_modalities_atomically` above: held for the
    // whole test.
    let _env_lock = medulla_kernel::crypto::acquire_test_env_lock().await;
    let dir = cm_dir("five-rollback");
    let points = vec![(1_000_000_000i64, vec![10.0])];
    let inner = Arc::new(RedbBackend::open(dir.clone(), 64).unwrap());
    let backend: Arc<dyn PersistenceBackend> = Arc::new(FailingBackend {
        inner: inner.clone(),
    });
    let state = new_state(Some(dir.clone()));
    {
        let mut s = state.write().await;
        let _ = s.registry.create_graph("media", GraphType::Global, None);
        s.persistence = Some(backend.clone());
    }

    let txn_id =
        stage_five_modalities(&state, "media", "m1", "sha256:def", "sensor", &points).await;

    // COMMIT must FAIL (the durable barrier errored) → error Response, no ack.
    let resp = txn_handle(
        &state,
        7,
        None,
        Method::Commit {
            txn_id,
            idempotency_key: None,
        },
    )
    .await
    .unwrap();
    assert!(resp.error.is_some(), "commit surfaces the durable failure");
    assert!(resp.result.is_none(), "no Bool ack on a failed commit");

    // NO PARTIAL COMMIT: none of the graph modalities applied in-memory.
    {
        let s = state.read().await;
        let core = s.registry.get("media").unwrap().core.clone();
        assert!(!core.has_node("m1"), "node rolled back");
        assert!(!core.has_node("<urn:a>"), "CONSTRUCT triple rolled back");
        assert_eq!(core.semantic_store.read().len(), 0, "vector rolled back");
    }

    inner.shutdown();
    drop(inner);
    drop(backend);

    // And the measurement never landed durably either (the wtx never committed).
    {
        let shard =
            Shard::open(std::path::Path::new(&dir).join(shard_filename(0)).as_path()).unwrap();
        let read = shard.control_read().unwrap();
        let key = direct_test_series_key("media", "sensor");
        assert!(
            medulla_tsdb::store::meta_in_rtx(&read, &key)
                .unwrap()
                .is_none(),
            "series never landed durably"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// MEDULLA-P0-4 (CONCEPT:MEDULLA-KG.backend.ts-served-materialize) — the canonical time-series
/// read-path unification proof. A measurement committed through the cross-modal txn
/// path (staged alongside a plain node write in ONE txn — a measurement alone already
/// makes a txn cross-modal per `GraphTxnState::is_cross_modal`) is:
///  1. durable in the authoritative shard's SERIES tables (the atomic barrier,
///     unchanged — verified directly against that file, exactly like
///     `five_modality_atomic_commit`);
///  2. ALSO visible through the PUBLIC `Method::TsRange` read path immediately after
///     `Commit` acks — the actual gap this workstream closes (before, a cross-modal
///     measurement was durable yet permanently unreachable from `TsRange`/`TsScan`);
///  3. STILL visible via `TsRange` after a full process restart (drop + reopen BOTH
///     the redb backend AND the served time-series store from the same persist dir,
///     on a brand-new `ServerState`) — proving the served-store materialization is
///     itself a committed durable write, not an in-memory-only mirror that a restart
///     would lose.
#[cfg(feature = "tsdb")]
#[tokio::test(flavor = "multi_thread")]
async fn crossmodal_measurement_visible_via_public_tsrange_post_commit_and_restart() {
    use medulla_tsdb::store::SeriesStore;
    use medulla_protocol_types::protocol::ResultPayload;

    const SECRET: &str = "ts-unify-secret";
    // Held for the whole test — this one literally reopens the backend
    // ("...post_commit_and_restart"), so cipher stability across BOTH opens is
    // exactly what this lock guarantees. See
    // `crossmodal_txn_commits_all_modalities_atomically` above.
    let _env_lock = medulla_kernel::crypto::acquire_test_env_lock().await;
    let dir = cm_dir("ts-unify");
    let points = vec![
        (1_000_000_000i64, vec![10.0]),
        (2_000_000_000i64, vec![20.0]),
        (3_000_000_000i64, vec![30.0]),
    ];

    let backend: Arc<dyn PersistenceBackend> =
        Arc::new(RedbBackend::open(dir.clone(), 64).unwrap());
    let series_store = Arc::new(
        SeriesStore::open_in_dir(
            std::path::Path::new(&dir),
            medulla_kernel::store_authority::process_verifier(),
            medulla_kernel::store_authority::process_authority().principal(),
            &medulla_kernel::store_authority::process_authority().proof(),
        )
        .unwrap(),
    );
    let state = new_state(Some(dir.clone()));
    {
        let mut s = state.write().await;
        s.auth_secret = SECRET.to_string();
        let _ = s.registry.create_graph("media", GraphType::Global, None);
        s.persistence = Some(backend.clone());
        s.tsdb_store = Some(series_store.clone());
    }

    let req = |id: u64, method: Method| current_request(SECRET, id, "media", method);

    // Stage a node + a measurement in ONE cross-modal txn, then commit.
    let begin = dispatch_on_heap(
        &state,
        req(
            1,
            Method::BeginTxn {
                graph: Some("media".to_string()),
                isolation: None,
            },
        ),
    )
    .await;
    let txn_id = match begin.result {
        Some(ResultPayload::String(id)) => id,
        other => panic!("BeginTxn id, got {other:?}"),
    };
    let r = dispatch_on_heap(
        &state,
        req(
            2,
            Method::TxnAddNode {
                txn_id: txn_id.clone(),
                node_id: "m1".into(),
                properties_msgpack: props(serde_json::json!({"type": "Sensor"})),
                graph: None,
            },
        ),
    )
    .await;
    assert!(r.error.is_none(), "stage node: {:?}", r.error);
    let r = dispatch_on_heap(
        &state,
        req(
            3,
            Method::TxnAddMeasurement {
                txn_id: txn_id.clone(),
                series: "sensor.ts-unify".to_string(),
                // Encoded inline (NOT the `sparql`-gated `meas_points` helper) so this
                // test only needs `tsdb`, matching the `#[cfg]` above.
                points: rmp_serde::to_vec(&points).unwrap(),
                graph: None,
            },
        ),
    )
    .await;
    assert!(r.error.is_none(), "stage measurement: {:?}", r.error);

    let commit = dispatch_on_heap(
        &state,
        req(
            4,
            Method::Commit {
                txn_id,
                idempotency_key: None,
            },
        ),
    )
    .await;
    assert_eq!(
        as_bool(commit),
        Some(true),
        "cross-modal commit must succeed"
    );

    // (1) POST-COMMIT visibility through the PUBLIC TsRange read path (the served
    // series.redb) — the actual gap this workstream closes. Checked FIRST, while
    // `backend`/`series_store` are both still live (they're two independent redb
    // files/handles, so no lock conflict).
    let ts_range = || {
        req(
            5,
            Method::TsRange {
                series_id: "sensor.ts-unify".to_string(),
                from: 0,
                to: i64::MAX,
            },
        )
    };
    let decode_ts = |r: Response| -> Vec<(i64, Vec<f64>)> {
        assert!(r.error.is_none(), "TsRange error: {:?}", r.error);
        match r.result {
            Some(ResultPayload::Raw(bytes)) => rmp_serde::from_slice(&bytes).unwrap(),
            other => panic!("expected Raw TsRange result, got {other:?}"),
        }
    };
    let got = decode_ts(dispatch_on_heap(&state, ts_range()).await);
    assert_eq!(
        got, points,
        "measurement committed via the cross-modal txn path must be visible through \
         the PUBLIC TsRange API immediately after Commit"
    );

    // (2) Durable in the authoritative shard too — the atomic barrier.
    // redb holds an EXCLUSIVE per-process file lock, so `backend` (which owns the
    // live shard handle) must release it first — exactly the ordering
    // `five_modality_atomic_commit` uses to open a second, direct handle on the
    // same file.
    backend.shutdown();
    drop(backend);
    {
        let shard =
            Shard::open(std::path::Path::new(&dir).join(shard_filename(0)).as_path()).unwrap();
        let read = shard.control_read().unwrap();
        let key = envelope_test_series_key("media", "sensor.ts-unify");
        let meta = medulla_tsdb::store::meta_in_rtx(&read, &key)
            .unwrap()
            .expect("measurement durable in the authoritative shard");
        assert_eq!(meta.count, 3, "all 3 points durable in the shard");
    }

    // (3) RESTART: drop + reopen BOTH stores from the SAME persist dir on a FRESH
    // ServerState, then re-run the SAME public TsRange call.
    drop(series_store);
    drop(state);

    let backend2: Arc<dyn PersistenceBackend> =
        Arc::new(RedbBackend::open(dir.clone(), 64).unwrap());
    let state2 = new_state(Some(dir.clone()));
    backend2.load_all(&state2).await.unwrap();
    let series_store2 = Arc::new(
        SeriesStore::open_in_dir(
            std::path::Path::new(&dir),
            medulla_kernel::store_authority::process_verifier(),
            medulla_kernel::store_authority::process_authority().principal(),
            &medulla_kernel::store_authority::process_authority().proof(),
        )
        .unwrap(),
    );
    {
        let mut s = state2.write().await;
        s.auth_secret = SECRET.to_string();
        s.persistence = Some(backend2.clone());
        s.tsdb_store = Some(series_store2.clone());
    }

    let got2 = decode_ts(dispatch_on_heap(&state2, ts_range()).await);
    assert_eq!(
        got2, points,
        "measurement must STILL be visible through the PUBLIC TsRange API after a \
         full restart (served store reopened from disk, not rebuilt from RAM)"
    );

    backend2.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

/// L16 startup reconciliation (CONCEPT:MEDULLA-KG.backend.ts-startup-reconcile) — the proof that closes the
/// MEDULLA-P0-4 residual. Simulates a crash STRICTLY BETWEEN the two commits by landing a
/// measurement batch in the authoritative shard via the low-level `commit_crossmodal` (the atomic
/// barrier alone) WITHOUT going through `medulla_h_txn::txn::commit_cross_modal_txn` (which
/// is what performs the served-store replay) — so the served `series.redb` never sees
/// it, exactly the documented gap. Asserts:
///  1. Before reconciliation, the measurement is invisible via the PUBLIC `TsRange`.
///  2. `RedbBackend::reconcile_time_series` finds + replays it; `TsRange` now returns
///     it.
///  3. Running reconciliation a SECOND time is a true no-op (nothing reconciled) and
///     does NOT duplicate the points — proving idempotency.
#[cfg(feature = "tsdb")]
#[tokio::test(flavor = "multi_thread")]
async fn startup_reconciliation_closes_the_crash_window_gap() {
    use medulla_tsdb::store::SeriesStore;
    use medulla_protocol_types::protocol::ResultPayload;

    const SECRET: &str = "ts-reconcile-secret";
    // See `crossmodal_txn_commits_all_modalities_atomically` above: held for the
    // whole test.
    let _env_lock = medulla_kernel::crypto::acquire_test_env_lock().await;
    let dir = cm_dir("ts-reconcile");
    let points: Vec<(i64, Vec<f64>)> = vec![
        (1_000_000_000i64, vec![1.0]),
        (2_000_000_000i64, vec![2.0]),
        (3_000_000_000i64, vec![3.0]),
    ];
    const SERIES: &str = "sensor.ts-reconcile";
    const BUCKET_NS: u64 = 3_600_000_000_000; // 1h — matches DEFAULT_MEASUREMENT_BUCKET_NS.

    let backend: Arc<dyn PersistenceBackend> =
        Arc::new(RedbBackend::open(dir.clone(), 64).unwrap());
    let series_store = Arc::new(
        SeriesStore::open_in_dir(
            std::path::Path::new(&dir),
            medulla_kernel::store_authority::process_verifier(),
            medulla_kernel::store_authority::process_authority().principal(),
            &medulla_kernel::store_authority::process_authority().proof(),
        )
        .unwrap(),
    );
    let state = new_state(Some(dir.clone()));
    {
        let mut s = state.write().await;
        s.auth_secret = SECRET.to_string();
        let _ = s.registry.create_graph("media", GraphType::Global, None);
        s.persistence = Some(backend.clone());
        s.tsdb_store = Some(series_store.clone());
    }

    // ── Simulate the crash window: land the measurement ONLY in the authoritative
    // shard, via the
    // atomic barrier alone, bypassing the txn-handler replay step entirely. ──
    let measurement: medulla_store::MeasurementBatch = (
        envelope_test_series_key("media", SERIES),
        1,
        BUCKET_NS,
        vec!["value".to_string()],
        points.clone(),
    );
    backend
        .commit_crossmodal("media", &[], &[], &[], std::slice::from_ref(&measurement))
        .await
        .expect("authoritative-shard-only commit must succeed");

    let req = |id: u64, method: Method| current_request(SECRET, id, "media", method);
    let ts_range = |id: u64| {
        req(
            id,
            Method::TsRange {
                series_id: SERIES.to_string(),
                from: 0,
                to: i64::MAX,
            },
        )
    };
    let decode_ts = |r: Response| -> Vec<(i64, Vec<f64>)> {
        assert!(r.error.is_none(), "TsRange error: {:?}", r.error);
        match r.result {
            Some(ResultPayload::Raw(bytes)) => rmp_serde::from_slice(&bytes).unwrap(),
            other => panic!("expected Raw TsRange result, got {other:?}"),
        }
    };

    // (1) BEFORE reconciliation: invisible through the PUBLIC TsRange — the gap
    // `commit_crossmodal` alone (without the txn-handler replay step) leaves open.
    // (Durability in the authoritative shard itself is proven by reconciliation
    // finding + replaying exactly 3 points — a SECOND `Database` handle can't be
    // opened directly on the shard here to double-check, since `backend` still
    // holds redb's exclusive per-process file lock on it, same constraint the
    // MEDULLA-P0-4 test works around by dropping its backend first.)
    let before = decode_ts(dispatch_on_heap(&state, ts_range(1)).await);
    assert!(
        before.is_empty(),
        "before reconciliation, a measurement landed only via the authoritative shard \
         commit (the simulated crash) must NOT yet be visible through TsRange"
    );

    // (2) Reconcile: the redb-backed reader is reached the SAME way production code
    // does — downcast the trait object via `as_redb()`.
    let redb = backend.as_redb().expect("redb backend");
    let report = redb
        .reconcile_time_series(&series_store)
        .await
        .expect("reconciliation must succeed");
    assert_eq!(
        report.series_reconciled, 1,
        "exactly one series needed replay"
    );
    assert_eq!(report.points_replayed, 3, "all 3 points replayed");

    let after = decode_ts(dispatch_on_heap(&state, ts_range(2)).await);
    assert_eq!(
        after, points,
        "after reconciliation, the measurement must be visible through the PUBLIC \
         TsRange API — the crash-window gap is closed"
    );

    // (3) IDEMPOTENCY: reconciling again is a true no-op — nothing to replay, and
    // TsRange returns the SAME points (no duplicates).
    let report2 = redb
        .reconcile_time_series(&series_store)
        .await
        .expect("second reconciliation must succeed");
    assert_eq!(
        report2.series_reconciled, 0,
        "a converged series must not be re-reconciled"
    );
    assert_eq!(report2.points_replayed, 0);
    let after2 = decode_ts(dispatch_on_heap(&state, ts_range(3)).await);
    assert_eq!(
        after2, points,
        "running reconciliation twice must not duplicate any point"
    );

    backend.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}
