//! Crash-injected and racing proofs of the commit-before-ack fence
//! (MEDULLA-OBJECT-STORAGE-BACKEND-R003[.1]).
//!
//! All proving state is the object root: a crash between the segment upload
//! and the `HEAD` conditional put leaves `HEAD` whole at the prior version
//! with the unacked write absent, and two racing publishers fence to one
//! `HEAD`. Every store here is a test fixture of the loopback object store;
//! no production type is added.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use futures_util::stream::BoxStream;
use object_store::memory::InMemory;
use object_store::path::Path as ObjPath;
use object_store::{ObjectStore, ObjectStoreExt, PutMode, PutOptions, Result as OsResult};
use rmp_serde::to_vec_named;
use sha2::{Digest as _, Sha256};

// spec: MEDULLA-OBJECT-STORAGE-BACKEND-R003[.1]
use crate::object_store_backend::ObjectStoreBackend;
use medulla_protocol_types::protocol::GraphType;
use medulla_storage::object::commit::{CommitRefusal, GraphBootstrap};
use medulla_storage::object::fetch::{ObjectHead, SegmentHeader, VersionManifest};
use medulla_storage::{DurabilityClass, StorageBackendSpec};

const TENANT: &str = "t";
const GRAPH: &str = "g";
/// Digest of an empty snapshot segment (the bootstrapped snapshot of v0).
const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
const ROOT: &str = "obj-root";

/// Version zero of the test graph: its identity, no data, no snapshot.
fn bootstrap() -> Result<GraphBootstrap, CommitRefusal> {
    Ok(GraphBootstrap {
        graph_name: GRAPH.to_string(),
        graph_type: GraphType::Global,
        graph_version: 0,
        seal_key_id: None,
        snapshot: Vec::new(),
    })
}

fn base() -> String {
    format!("{ROOT}/{TENANT}/{GRAPH}")
}

fn head_bytes(version: u64) -> Vec<u8> {
    serde_json::to_vec(&ObjectHead {
        version,
        manifest: format!("manifests/{version:020}.json"),
    })
    .unwrap()
}

/// A crash injected as object-store failure: puts at the injected step error
/// while the fault is armed. `fail_manifest=true` crashes a publish after
/// the segment upload (between steps 2 and 3); `fail_head=true` crashes it
/// after the create-only manifest put (between steps 3 and 4). Somewhere a
/// store in production code: this is the exact window the row's evidence
/// names, and no production type is added for it.
#[derive(Debug)]
struct CrashyStore {
    inner: InMemory,
    fault: AtomicBool,
    fail_manifest: bool,
    fail_head: bool,
}

fn crash_error() -> object_store::Error {
    object_store::Error::Generic {
        store: "crashy",
        source: Box::new(std::io::Error::other("crash between publish steps")),
    }
}

impl CrashyStore {
    fn new(fail_manifest: bool, fail_head: bool) -> Arc<Self> {
        Arc::new(Self {
            inner: InMemory::new(),
            fault: AtomicBool::new(false),
            fail_manifest,
            fail_head,
        })
    }

    fn arm(&self) {
        self.fault.store(true, Ordering::SeqCst);
    }

    fn disarm(&self) {
        self.fault.store(false, Ordering::SeqCst);
    }
}

impl std::fmt::Display for CrashyStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("crashy-store")
    }
}

#[async_trait]
impl ObjectStore for CrashyStore {
    async fn put_opts(
        &self,
        location: &ObjPath,
        payload: object_store::PutPayload,
        options: PutOptions,
    ) -> OsResult<object_store::PutResult> {
        let p = location.as_ref();
        if self.fault.load(Ordering::SeqCst)
            && ((self.fail_head && p.ends_with("HEAD"))
                || (self.fail_manifest && p.contains("manifests/")))
        {
            return Err(crash_error());
        }
        self.inner.put_opts(location, payload, options).await
    }

    async fn put_multipart_opts(
        &self,
        location: &ObjPath,
        options: object_store::PutMultipartOptions,
    ) -> OsResult<Box<dyn object_store::MultipartUpload>> {
        self.inner.put_multipart_opts(location, options).await
    }

    async fn get_opts(
        &self,
        location: &ObjPath,
        options: object_store::GetOptions,
    ) -> OsResult<object_store::GetResult> {
        self.inner.get_opts(location, options).await
    }

    fn delete_stream(
        &self,
        locations: BoxStream<'static, OsResult<ObjPath>>,
    ) -> BoxStream<'static, OsResult<ObjPath>> {
        self.inner.delete_stream(locations)
    }

    fn list(
        &self,
        prefix: Option<&ObjPath>,
    ) -> BoxStream<'static, OsResult<object_store::ObjectMeta>> {
        self.inner.list(prefix)
    }

    async fn list_with_delimiter(
        &self,
        prefix: Option<&ObjPath>,
    ) -> OsResult<object_store::ListResult> {
        self.inner.list_with_delimiter(prefix).await
    }

    async fn copy_opts(
        &self,
        from: &ObjPath,
        to: &ObjPath,
        options: object_store::CopyOptions,
    ) -> OsResult<()> {
        self.inner.copy_opts(from, to, options).await
    }
}

fn hex_digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn manifest_bytes(v: u64, parent: Option<u64>, batches: Vec<SegmentHeader>) -> Vec<u8> {
    let mut manifest = VersionManifest {
        schema_version: 3,
        tenant: TENANT.to_string(),
        graph: GRAPH.to_string(),
        version: v,
        parent_version: parent,
        graph_name: GRAPH.to_string(),
        graph_type: GraphType::Global,
        graph_version: v,
        seal_key_id: None,
        snapshot: SegmentHeader {
            key: EMPTY_SHA256.to_string(),
            sha256: EMPTY_SHA256.to_string(),
        },
        batches,
        digest: String::new(),
    };
    manifest.digest = manifest.body_digest().unwrap();
    serde_json::to_vec(&manifest).unwrap()
}

/// Bootstrap the whole prior version the fence builds on: snapshot segment,
/// manifest v0 (cumulative journal empty), and `HEAD` at v0.
async fn seed(store: &InMemory) {
    store
        .put(
            &ObjPath::from(format!("{}/segments/{EMPTY_SHA256}.seg", base())),
            Vec::new().into(),
        )
        .await
        .unwrap();
    store
        .put_opts(
            &ObjPath::from(format!("{}/manifests/{:020}.json", base(), 0)),
            manifest_bytes(0, None, Vec::new()).into(),
            PutOptions {
                mode: PutMode::Create,
                ..PutOptions::default()
            },
        )
        .await
        .unwrap();
    store
        .put_opts(
            &ObjPath::from(format!("{}/HEAD", base())),
            head_bytes(0).into(),
            PutOptions {
                mode: PutMode::Create,
                ..PutOptions::default()
            },
        )
        .await
        .unwrap();
}

fn backend(store: Arc<dyn ObjectStore>) -> ObjectStoreBackend {
    ObjectStoreBackend::open(
        &StorageBackendSpec::ObjectStore {
            endpoint: None,
            bucket: None,
            root: None,
            durability: Some(DurabilityClass::ObjectStoreConditionalPut),
            layout_digest: None,
        },
        store,
        ROOT,
    )
    .unwrap()
}

async fn head(store: &InMemory) -> ObjectHead {
    let r = store
        .get(&ObjPath::from(format!("{}/HEAD", base())))
        .await
        .unwrap();
    serde_json::from_slice(&r.bytes().await.unwrap()).unwrap()
}

async fn object_absent(store: &InMemory, path: &str) -> bool {
    store.get(&ObjPath::from(path)).await.is_err()
}

// spec: MEDULLA-OBJECT-STORAGE-BACKEND-R003.1
// Crash between steps 2 and 3 (segment uploaded, manifest and HEAD not):
// the unacked write stays absent from the whole prior version and a reopened
// publisher resumes exactly where the fence left off.
#[tokio::test]
async fn commit_before_ack_and_partial_publish() {
    let crashy = CrashyStore::new(true, false);
    seed(&crashy.inner).await;
    let backend = backend(crashy.clone());

    // One acked commit: steps 2–4 complete; the ack carried the new HEAD.
    let batch_a = to_vec_named(&["batch-a-rows"]).unwrap();
    let ack_a = backend
        .commit_mutation_batch(TENANT, GRAPH, &batch_a, None, &bootstrap)
        .await
        .unwrap();
    assert_eq!(ack_a.version, 1);
    assert_eq!(head(&crashy.inner).await.version, 1);

    // Crash after the segment upload of a second batch: the publish refuses
    // (the caller was never acked), HEAD stays whole at the prior version,
    // and the unacked write is absent from it: no manifest that would
    // include it, no HEAD naming it.
    let batch_b = to_vec_named(&["batch-b-rows"]).unwrap();
    crashy.arm();
    let refusal = backend
        .commit_mutation_batch(TENANT, GRAPH, &batch_b, None, &bootstrap)
        .await
        .unwrap_err();
    assert_eq!(refusal.wire_code(), "OBJECT_HEAD_MISSING");
    assert_eq!(head(&crashy.inner).await.version, 1);
    assert!(
        object_absent(
            &crashy.inner,
            &format!("{}/manifests/{:020}.json", base(), 2)
        )
        .await
    );

    // Restart: the crash clears and a reopened publisher retries the same
    // commit. It lands with the whole prior version as its parent and is
    // acked at exactly the next version of the fence.
    crashy.disarm();
    let ack_b = backend
        .commit_mutation_batch(TENANT, GRAPH, &batch_b, None, &bootstrap)
        .await
        .unwrap();
    assert_eq!(ack_b.version, 2);
    assert_eq!(head(&crashy.inner).await.version, 2);
    assert!(
        !object_absent(
            &crashy.inner,
            &format!("{}/segments/{}.seg", base(), hex_digest(&batch_b))
        )
        .await
    );
}

// spec: MEDULLA-OBJECT-STORAGE-BACKEND-R003
// Crash between steps 3 and 4 (create-only manifest landed, HEAD fence not):
// HEAD stays whole at the prior version, and the retry that re-fences on it
// loses the next version's create to the orphan and refuses OBJECT_PUBLISH_STALE —
// the node must discard its local commit and re-fetch HEAD before publishing.
#[tokio::test]
async fn crash_after_manifest_before_head_refuses_with_stale() {
    let crashy = CrashyStore::new(false, true);
    seed(&crashy.inner).await;
    let backend = backend(crashy.clone());

    let batch = to_vec_named(&["late-rows"]).unwrap();
    crashy.arm();
    let refusal = backend
        .commit_mutation_batch(TENANT, GRAPH, &batch, None, &bootstrap)
        .await
        .unwrap_err();
    assert_eq!(refusal.wire_code(), "OBJECT_HEAD_MISSING");
    assert_eq!(head(&crashy.inner).await.version, 0);
    assert!(
        !object_absent(
            &crashy.inner,
            &format!("{}/manifests/{:020}.json", base(), 1)
        )
        .await
    );

    crashy.disarm();
    let refusal = backend
        .commit_mutation_batch(TENANT, GRAPH, &batch, None, &bootstrap)
        .await
        .unwrap_err();
    assert_eq!(refusal, CommitRefusal::PublishStale { version: 0 });
    assert_eq!(head(&crashy.inner).await.version, 0);
}

// spec: MEDULLA-OBJECT-STORAGE-BACKEND-R003
// Two publishers racing the same fence: the ones that lose the conditional
// put refuse OBJECT_PUBLISH_STALE and never advance HEAD, the root has
// exactly one HEAD document, and the graph version advances once per
// fence-win.
#[tokio::test]
async fn racy_conditional_puts_fence_one_head() {
    let store = Arc::new(InMemory::new());
    seed(&store).await;
    let backend = backend(store.clone());

    let batch = to_vec_named(&["raced-rows"]).unwrap();
    let (ra, rb) = tokio::join!(
        backend.commit_mutation_batch(TENANT, GRAPH, &batch, None, &bootstrap),
        backend.commit_mutation_batch(TENANT, GRAPH, &batch, None, &bootstrap)
    );
    match (ra, rb) {
        (Ok(_), Err(CommitRefusal::PublishStale { .. }))
        | (Err(CommitRefusal::PublishStale { .. }), Ok(_)) => {
            // One fence-win: HEAD advanced exactly one version.
            assert_eq!(head(&store).await.version, 1);
        }
        (Ok(a), Ok(b)) => {
            // The second publisher read the first one's fresh HEAD before the
            // fence and legitimately published the next version.
            let mut versions = vec![a.version, b.version];
            versions.sort_unstable();
            assert_eq!(versions, vec![1, 2]);
            assert_eq!(head(&store).await.version, 2);
        }
        _ => panic!("the fence must let exactly the winner advance HEAD"),
    }
    assert!(head(&store).await.version >= 1);
}

// spec: MEDULLA-OBJECT-STORAGE-BACKEND-R003
#[tokio::test]
async fn undeclared_durability_class_refuses_open() {
    let err = ObjectStoreBackend::open(
        &StorageBackendSpec::ObjectStore {
            endpoint: None,
            bucket: None,
            root: None,
            durability: None,
            layout_digest: None,
        },
        Arc::new(InMemory::new()),
        ROOT,
    )
    .unwrap_err();
    assert_eq!(err.wire_code(), "OBJECT_DURABILITY_UNDECLARED");
}

// spec: MEDULLA-OBJECT-STORAGE-BACKEND-R004
// Two nodes over one shared root: each fetches the `HEAD` version in one pass and reads
// identical state, and a commit by either node is read by the other. The root is the only
// shared state; the store layer adds no second consensus.
#[tokio::test]
async fn two_nodes_read_identical_state() {
    use medulla_storage::object::fetch::ObjectFetcher;

    let store = Arc::new(InMemory::new());
    seed(&store).await;
    let node_a = backend(store.clone());
    let node_b = backend(store.clone());
    let reader_a = ObjectFetcher::new(store.clone(), ROOT);
    let reader_b = ObjectFetcher::new(store.clone(), ROOT);

    let batch_one = to_vec_named(&["node-a-rows"]).unwrap();
    let batch_two = to_vec_named(&["node-a-more-rows"]).unwrap();
    assert_eq!(
        node_a
            .commit_mutation_batch(TENANT, GRAPH, &batch_one, None, &bootstrap)
            .await
            .unwrap()
            .version,
        1
    );
    assert_eq!(
        node_a
            .commit_mutation_batch(TENANT, GRAPH, &batch_two, None, &bootstrap)
            .await
            .unwrap()
            .version,
        2
    );

    // A node with no local copy fetches `HEAD` and its segments and sees the same version.
    let head_a = reader_a.head(TENANT, GRAPH).await.unwrap();
    let head_b = reader_b.head(TENANT, GRAPH).await.unwrap();
    assert_eq!(head_a, head_b);
    assert_eq!(head_a.version, 2);
    let state_a = reader_a.materialize(TENANT, GRAPH, head_a.version).await.unwrap();
    let state_b = reader_b.materialize(TENANT, GRAPH, head_b.version).await.unwrap();
    assert_eq!(state_a, state_b);
    assert_eq!(state_a.batches.len(), 2);
    assert_eq!(state_a.batches[0].bytes, batch_one);
    assert_eq!(state_a.batches[1].bytes, batch_two);

    // The second node commits next on the same root; the first node reads it.
    let batch_three = to_vec_named(&["node-b-rows"]).unwrap();
    assert_eq!(
        node_b
            .commit_mutation_batch(TENANT, GRAPH, &batch_three, None, &bootstrap)
            .await
            .unwrap()
            .version,
        3
    );
    let head_a = reader_a.head(TENANT, GRAPH).await.unwrap();
    assert_eq!(head_a.version, 3);
    let state_a = reader_a.materialize(TENANT, GRAPH, head_a.version).await.unwrap();
    let state_b = reader_b.materialize(TENANT, GRAPH, 3).await.unwrap();
    assert_eq!(state_a, state_b);
    assert_eq!(state_a.batches.len(), 3);
}

// spec: MEDULLA-OBJECT-STORAGE-BACKEND-R002.2
// Secret references resolve from named environment variables; each refusal names its
// code and happens before any request to the store. Variable names are unique per case,
// so the test is safe under parallel tests.
#[test]
fn client_resolves_secret_refs_from_the_environment() {
    use crate::object_store_backend::resolve_object_store_client;
    use medulla_storage::SecretRef;

    fn spec(case: &str, with_root: bool) -> StorageBackendSpec {
        StorageBackendSpec::ObjectStore {
            endpoint: Some(SecretRef(format!("R2_2_{case}_ENDPOINT"))),
            bucket: Some(SecretRef(format!("R2_2_{case}_BUCKET"))),
            root: with_root.then(|| SecretRef(format!("R2_2_{case}_ROOT"))),
            durability: Some(DurabilityClass::ObjectStoreConditionalPut),
            layout_digest: None,
        }
    }
    fn set(case: &str, endpoint: &str) {
        std::env::set_var(format!("R2_2_{case}_ENDPOINT"), endpoint);
        std::env::set_var(format!("R2_2_{case}_BUCKET"), "bucket-a");
        std::env::set_var(format!("R2_2_{case}_ROOT"), "root-a/prefix");
    }

    set("OK", "http://127.0.0.1:9000");
    let (_client, root) = resolve_object_store_client(&spec("OK", true)).unwrap();
    assert_eq!(root, "root-a/prefix");

    set("HTTPS", "https://s3.example.invalid");
    assert!(resolve_object_store_client(&spec("HTTPS", true)).is_ok());

    set("UNSET", "http://127.0.0.1:9000");
    std::env::remove_var("R2_2_UNSET_BUCKET");
    let err = resolve_object_store_client(&spec("UNSET", true)).err().unwrap();
    assert_eq!(err.wire_code(), "OBJECT_STORE_SECRET_UNRESOLVED");
    assert_eq!(
        err,
        crate::object_store_backend::ObjectStoreOpenRefusal::SecretUnresolved { field: "bucket" }
    );

    set("PLAIN", "http://s3.example.invalid:9000");
    let err = resolve_object_store_client(&spec("PLAIN", true)).err().unwrap();
    assert_eq!(err.wire_code(), "OBJECT_STORE_PLAIN_HTTP_REFUSED");

    set("PG", "postgres://db.example.invalid/x");
    let err = resolve_object_store_client(&spec("PG", true)).err().unwrap();
    assert_eq!(err.wire_code(), "EXTERNAL_PLACEMENT_FORBIDDEN");

    set("NOROOT", "http://127.0.0.1:9000");
    let err = resolve_object_store_client(&spec("NOROOT", false)).err().unwrap();
    assert_eq!(err.wire_code(), "OBJECT_STORE_SECRET_UNRESOLVED");
}

// spec: MEDULLA-OBJECT-STORAGE-BACKEND-R002
// The first publish of a graph with no `HEAD` bootstraps version zero, the cold open
// lists the graph, and the head version materializes the published batch.
#[tokio::test]
async fn first_publish_bootstraps_and_the_cold_open_lists_the_graph() {
    let store = Arc::new(InMemory::new());
    let backend = backend(store.clone());
    let segment = to_vec_named(&["batch-a-rows"]).unwrap();
    let ack = backend
        .commit_mutation_batch(TENANT, GRAPH, &segment, None, &bootstrap)
        .await
        .unwrap();
    assert_eq!(ack.version, 1);
    let fetcher = medulla_storage::object::fetch::ObjectFetcher::new(store.clone(), ROOT);
    assert_eq!(fetcher.graphs(TENANT).await.unwrap(), [GRAPH.to_string()]);
    let version = fetcher.materialize(TENANT, GRAPH, 1).await.unwrap();
    assert!(version.snapshot.bytes.is_empty());
    assert_eq!(version.batches.len(), 1);
    assert_eq!(version.batches[0].bytes, segment);
}

// spec: MEDULLA-OBJECT-STORAGE-BACKEND-R003
// A graph that already holds data bootstraps version zero from its owner-table
// snapshot, and the manifest records the graph's name, type and version, which the
// read path returns as written (no inference).
#[tokio::test]
async fn bootstrap_writes_the_snapshot_the_graph_type_and_the_graph_version() {
    let store = Arc::new(InMemory::new());
    let backend = backend(store.clone());
    let segment = to_vec_named(&["batch-b-rows"]).unwrap();
    let snapshot = b"owner-table-dump".to_vec();
    let with_data = || {
        Ok(GraphBootstrap {
            graph_name: "named-graph".to_string(),
            graph_type: GraphType::Commons,
            graph_version: 7,
            seal_key_id: None,
            snapshot: snapshot.clone(),
        })
    };
    let ack = backend
        .commit_mutation_batch(TENANT, GRAPH, &segment, Some(8), &with_data)
        .await
        .unwrap();
    assert_eq!(ack.version, 1);
    let fetcher = medulla_storage::object::fetch::ObjectFetcher::new(store, ROOT);
    let zero = fetcher.materialize(TENANT, GRAPH, 0).await.unwrap();
    assert_eq!(zero.snapshot.bytes, snapshot);
    assert_eq!(zero.graph_name, "named-graph");
    assert_eq!(zero.graph_type, GraphType::Commons);
    assert_eq!(zero.graph_version, 7);
    let head = fetcher.materialize(TENANT, GRAPH, 1).await.unwrap();
    assert_eq!(head.snapshot.bytes, snapshot);
    assert_eq!(head.graph_type, GraphType::Commons);
    assert_eq!(head.graph_version, 8);
}

// spec: MEDULLA-OBJECT-STORAGE-BACKEND-R002
#[cfg(feature = "redb")]
#[tokio::test]
async fn restore_refuses_a_root_sealed_under_a_key_the_store_lacks() {
    let store = Arc::new(InMemory::new());
    let backend = backend(store.clone());
    let segment = to_vec_named(&["batch-rows"]).unwrap();
    let sealed = || {
        Ok(GraphBootstrap {
            graph_name: "sealed-graph".to_string(),
            graph_type: GraphType::Global,
            graph_version: 0,
            seal_key_id: Some("key-a".to_string()),
            snapshot: Vec::new(),
        })
    };
    backend
        .commit_mutation_batch(
            crate::object_store_backend::OBJECT_ROOT_TENANT,
            GRAPH,
            &segment,
            Some(1),
            &sealed,
        )
        .await
        .unwrap();
    let fetcher = medulla_storage::object::fetch::ObjectFetcher::new(store, ROOT);
    let head = fetcher
        .materialize(crate::object_store_backend::OBJECT_ROOT_TENANT, GRAPH, 1)
        .await
        .unwrap();
    assert_eq!(head.seal_key_id.as_deref(), Some("key-a"));

    let dir = tempfile::tempdir().unwrap();
    let redb = crate::redb_backend::RedbBackend::open(dir.path().to_string_lossy().to_string(), 64)
        .unwrap();
    let error = backend.restore_into(&redb).await.unwrap_err();
    assert!(error.starts_with("OBJECT_SEAL_KEY_MISSING"), "{error}");
    assert!(error.contains("key-a"), "{error}");
    assert!(error.contains("does not hold"), "{error}");
}

// spec: MEDULLA-OBJECT-STORAGE-BACKEND-R002.4
// The startup open reads the setting once and refuses before any effect. The cases share one
// process-wide variable, so they run in one test; the secret variables are unique to it.
#[test]
fn open_from_env_refuses_before_the_listener() {
    use crate::object_store_backend::{open_from_env, STORAGE_BACKEND_ENV};

    let spec = r#"{"variant":"object_store","endpoint":"R2_4_ENDPOINT","bucket":"R2_4_BUCKET","root":"R2_4_ROOT","durability":"object_store_conditional_put"}"#;

    // Unset, empty and `local` are the redb default: nothing is opened.
    std::env::remove_var(STORAGE_BACKEND_ENV);
    assert!(open_from_env().unwrap().is_none());
    for setting in ["", "local"] {
        std::env::set_var(STORAGE_BACKEND_ENV, setting);
        assert!(open_from_env().unwrap().is_none(), "{setting:?}");
    }

    // A spec whose secret variables are unset refuses naming the field, never a value.
    std::env::set_var(STORAGE_BACKEND_ENV, spec);
    for name in ["R2_4_ENDPOINT", "R2_4_BUCKET", "R2_4_ROOT"] {
        std::env::remove_var(name);
    }
    let refusal = open_from_env().err().expect("an unset secret refuses");
    assert!(
        refusal.starts_with("OBJECT_STORE_SECRET_UNRESOLVED"),
        "{refusal}"
    );

    // An unknown variant and an unreadable body are `STORAGE_BACKEND_UNKNOWN`.
    for setting in [r#"{"variant":"tape_library"}"#, "{"] {
        std::env::set_var(STORAGE_BACKEND_ENV, setting);
        let refusal = open_from_env().err().expect("an unknown spec refuses");
        assert!(refusal.starts_with("STORAGE_BACKEND_UNKNOWN"), "{refusal}");
    }

    // A resolvable spec opens: the same function the listener waits on.
    std::env::set_var(STORAGE_BACKEND_ENV, spec);
    std::env::set_var("R2_4_ENDPOINT", "http://127.0.0.1:9");
    std::env::set_var("R2_4_BUCKET", "bucket-a");
    std::env::set_var("R2_4_ROOT", "root-a");
    assert!(open_from_env().unwrap().is_some());
    std::env::remove_var(STORAGE_BACKEND_ENV);
}

/// One single-node batch of graph `GRAPH_FNAME`, committing at graph version `index + 1`.
#[cfg(feature = "redb")]
fn node_batch(index: u64) -> medulla_protocol_types::mutation_batch::MutationBatch {
    use medulla_mutation_types::mutation_scope::{
        DurabilityDomain, IncarnationId, LogicalName, MutationOutboxIntent, MutationScopeIdentity,
        MutationSurface, ScopeTenantId, MUTATION_BATCH_VERSION,
    };
    use medulla_protocol_types::mutation_batch::{
        MutationBatch, MutationOperation, VersionExpectation, MUTATION_ACTOR_HEADER,
    };
    use medulla_protocol_types::protocol::Method;

    let identity = MutationScopeIdentity::graph(
        ScopeTenantId::new("restore-tenant").unwrap(),
        LogicalName::new(GRAPH_FNAME).unwrap(),
        IncarnationId::new("incarnation:test:object-restore").unwrap(),
    );
    let batch_id = format!("b{index}");
    let actor = format!("principal:sha256:{}", "b".repeat(64));
    let mut batch = MutationBatch {
        schema_version: MUTATION_BATCH_VERSION,
        batch_id: batch_id.clone(),
        envelope: medulla_store::redb_store::store_graph_names::fixture_operation_envelope(
            &identity, &actor, 17, &batch_id,
        ),
        identity,
        placement_epoch: 1,
        version_expectation: VersionExpectation::Graph(index),
        fencing_token: Some(1),
        authoritative_state: None,
        operations: vec![MutationOperation {
            ordinal: 0,
            surface: MutationSurface::Transaction,
            domain: DurabilityDomain::GraphRows,
            method: Method::AddNode {
                node_id: format!("node-{index}"),
                properties_msgpack: rmp_serde::to_vec_named(&serde_json::json!({"n": index}))
                    .unwrap(),
            },
        }],
        outbox: vec![MutationOutboxIntent {
            topic: "projection.restore".to_string(),
            key: batch_id,
            payload: rmp_serde::to_vec_named(&serde_json::json!({"event": index})).unwrap(),
            headers: std::collections::BTreeMap::from([(MUTATION_ACTOR_HEADER.to_string(), actor)]),
        }],
        created_at_ms: 10,
    };
    let schema_digest = batch
        .envelope
        .operation()
        .expect("fixture carries an operation envelope")
        .method_schema_digest;
    batch.reseal_envelope(schema_digest).unwrap();
    batch
}

#[cfg(feature = "redb")]
const GRAPH_FNAME: &str = "restore-graph";

// spec: MEDULLA-OBJECT-STORAGE-BACKEND-R002.5
// Three commits of one engine publish to the root; a fresh local store restores the head into
// itself, every batch present and the graph version installed, and a corrupted segment refuses
// the restore by digest.
#[cfg(feature = "redb")]
#[tokio::test]
async fn restore_materializes_head_into_an_empty_local_store() {
    use futures_util::TryStreamExt;
    use medulla_server_api::api::persistence::PersistenceBackend;

    let store = Arc::new(InMemory::new());
    let source_dir = tempfile::tempdir().unwrap();
    let mut source =
        crate::redb_backend::RedbBackend::open(source_dir.path().to_string_lossy().to_string(), 64)
            .unwrap();
    source
        .register_graph(GRAPH_FNAME, "restore-graph", GraphType::Commons)
        .await
        .unwrap();
    source
        .attach_object_store(Arc::new(backend(store.clone())))
        .unwrap();
    for index in 0..3 {
        PersistenceBackend::commit_mutation_batch(
            &source,
            GRAPH_FNAME,
            &node_batch(index),
            None,
            1_000 + index,
        )
        .await
        .expect("the batch commits and publishes");
    }

    // A fresh store restores the head: every published entry replays, and the graph version
    // is the head's.
    let target_dir = tempfile::tempdir().unwrap();
    let target =
        crate::redb_backend::RedbBackend::open(target_dir.path().to_string_lossy().to_string(), 64)
            .unwrap();
    let applied = backend(store.clone()).restore_into(&target).await.unwrap();
    assert_eq!(applied, 3);
    for index in 0..3 {
        let record =
            PersistenceBackend::read_mutation_batch(&target, GRAPH_FNAME, &format!("b{index}"))
                .await
                .unwrap();
        assert!(record.is_some(), "batch b{index} is in the restored ledger");
    }
    let (name, graph_type, version) = target.graph_identity(GRAPH_FNAME).unwrap().unwrap();
    assert_eq!((name.as_str(), graph_type, version), ("restore-graph", GraphType::Commons, 3));

    // A restore that finds the graph already at the head applies nothing.
    assert_eq!(backend(store.clone()).restore_into(&target).await.unwrap(), 0);

    // A corrupted segment refuses the restore by digest, and the root is never written.
    let segments: Vec<_> = store
        .list(Some(&ObjPath::from(ROOT)))
        .try_collect()
        .await
        .unwrap();
    let segment = segments
        .iter()
        .find(|meta| meta.location.as_ref().contains("/segments/"))
        .expect("the root holds a segment");
    store
        .put(&segment.location, b"corrupted".to_vec().into())
        .await
        .unwrap();
    let damaged_dir = tempfile::tempdir().unwrap();
    let damaged =
        crate::redb_backend::RedbBackend::open(damaged_dir.path().to_string_lossy().to_string(), 64)
            .unwrap();
    let refusal = backend(store).restore_into(&damaged).await.unwrap_err();
    assert!(
        refusal.starts_with("OBJECT_SEGMENT_DIGEST_MISMATCH"),
        "{refusal}"
    );
}

// spec: MEDULLA-OBJECT-STORAGE-BACKEND-R003.3
// The first commit on an empty root writes the snapshot, one batch segment, a create-only manifest
// and a create-only HEAD at version 1; the second publishes at version 2; a root whose next
// manifest was already taken refuses the third with OBJECT_PUBLISH_STALE.
#[tokio::test]
async fn first_commit_bootstraps_head_and_later_commits_publish() {
    let store = Arc::new(InMemory::new());
    let publisher = backend(store.clone());
    let snapshot = b"owner-table-dump".to_vec();
    let with_data = || {
        Ok(GraphBootstrap {
            graph_name: "named-graph".to_string(),
            graph_type: GraphType::Commons,
            graph_version: 0,
            seal_key_id: None,
            snapshot: snapshot.clone(),
        })
    };
    let first = to_vec_named(&["first-rows"]).unwrap();
    let ack = publisher
        .commit_mutation_batch(TENANT, GRAPH, &first, Some(1), &with_data)
        .await
        .unwrap();
    assert_eq!(ack.version, 1);
    assert_eq!(head(&store).await.version, 1);
    let fetcher = medulla_storage::object::fetch::ObjectFetcher::new(store.clone(), ROOT);
    let one = fetcher.materialize(TENANT, GRAPH, 1).await.unwrap();
    assert_eq!(one.snapshot.bytes, snapshot);
    assert_eq!(one.batches.len(), 1);
    assert_eq!(one.batches[0].bytes, first);

    let second = to_vec_named(&["second-rows"]).unwrap();
    let ack = publisher
        .commit_mutation_batch(TENANT, GRAPH, &second, Some(2), &with_data)
        .await
        .unwrap();
    assert_eq!(ack.version, 2);
    assert_eq!(head(&store).await.version, 2);
    let two = fetcher.materialize(TENANT, GRAPH, 2).await.unwrap();
    assert_eq!(two.batches.len(), 2);
    assert_eq!(two.batches[1].bytes, second);

    // Another node took version 3's manifest: this publisher's fence is stale.
    store
        .put(
            &ObjPath::from(format!("{}/manifests/{:020}.json", base(), 3)),
            manifest_bytes(3, Some(2), Vec::new()).into(),
        )
        .await
        .unwrap();
    let third = to_vec_named(&["third-rows"]).unwrap();
    let refusal = publisher
        .commit_mutation_batch(TENANT, GRAPH, &third, Some(3), &with_data)
        .await
        .unwrap_err();
    assert_eq!(refusal, CommitRefusal::PublishStale { version: 2 });
    assert_eq!(head(&store).await.version, 2);
}
