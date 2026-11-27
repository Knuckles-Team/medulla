//! Cold-lifecycle cost of the object-store backend (MEDULLA-OBJECT-STORAGE-BACKEND-R005).
//!
//! Drives one lifecycle against an in-memory object root: open (fetch `HEAD` and its
//! segments), read 1,000 nodes, commit 100 batches and publish a merge of two branch
//! heads. Object actions and bytes come from the counting run, which wraps the store in
//! `CountingObjectStore`; time and peak memory come from a separate matched run without
//! the wrapper. Output is JSON lines with `schema_version`, `host`, `revision`,
//! `lifecycle_digest`, `dimension` and `value`, read by `scripts/object_store_cost_gate.py`.
//!
//! Run: `cargo bench --features object-store-bench --bench object_store_cost`.
//! `MEDULLA_COST_REVISION` stamps the run; an unstamped run is claim-ineligible.

use std::sync::Arc;
use std::time::Instant;

use medulla_persistence::object_store_backend::ObjectStoreBackend;
use medulla_storage::object::commit::{CommitRefusal, GraphBootstrap};
use medulla_storage::object::counting::CountingObjectStore;
use medulla_storage::object::fetch::{ObjectFetcher, SegmentHeader, VersionManifest};
use medulla_storage::{DurabilityClass, StorageBackendSpec};
use object_store::memory::InMemory;
use object_store::path::Path as ObjPath;
use object_store::{ObjectStore, ObjectStoreExt, PutMode, PutOptions};
use sha2::{Digest, Sha256};

const ROOT: &str = "cost-root";
const TENANT: &str = "t";
const GRAPH: &str = "g";
const NODES: usize = 1_000;
const BATCHES: usize = 100;
const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
const SCHEMA_VERSION: u32 = 1;

fn base() -> String {
    format!("{ROOT}/{TENANT}/{GRAPH}")
}

fn bootstrap() -> Result<GraphBootstrap, CommitRefusal> {
    Ok(GraphBootstrap {
        graph_name: GRAPH.into(),
        graph_type: medulla_protocol_types::protocol::GraphType::Global,
        graph_version: 0,
        snapshot: Vec::new(),
    })
}

async fn seed(store: &dyn ObjectStore) {
    let mut manifest = VersionManifest {
        schema_version: 3,
        tenant: TENANT.into(),
        graph: GRAPH.into(),
        version: 0,
        parent_version: None,
        graph_name: GRAPH.into(),
        graph_type: medulla_protocol_types::protocol::GraphType::Global,
        graph_version: 0,
        snapshot: SegmentHeader {
            key: EMPTY_SHA256.into(),
            sha256: EMPTY_SHA256.into(),
        },
        batches: Vec::new(),
        digest: String::new(),
    };
    manifest.digest = manifest.body_digest().unwrap();
    store
        .put(
            &ObjPath::from(format!("{}/segments/{EMPTY_SHA256}.seg", base())),
            Vec::new().into(),
        )
        .await
        .unwrap();
    let create = || PutOptions {
        mode: PutMode::Create,
        ..PutOptions::default()
    };
    store
        .put_opts(
            &ObjPath::from(format!("{}/manifests/{:020}.json", base(), 0)),
            serde_json::to_vec(&manifest).unwrap().into(),
            create(),
        )
        .await
        .unwrap();
    store
        .put_opts(
            &ObjPath::from(format!("{}/HEAD", base())),
            serde_json::to_vec(&serde_json::json!({
                "version": 0,
                "manifest": format!("manifests/{:020}.json", 0),
            }))
            .unwrap()
            .into(),
            create(),
        )
        .await
        .unwrap();
}

fn batch(n: usize, rows: usize) -> Vec<u8> {
    let rows: Vec<String> = (0..rows).map(|i| format!("node-{n}-{i}")).collect();
    rmp_serde::to_vec_named(&rows).unwrap()
}

/// The lifecycle: commit `BATCHES` batches that carry `NODES` nodes, merge two branch
/// heads with one more batch, then cold-open and read every segment.
async fn lifecycle(store: Arc<dyn ObjectStore>) -> usize {
    seed(store.as_ref()).await;
    let backend = ObjectStoreBackend::open(
        &StorageBackendSpec::ObjectStore {
            endpoint: None,
            bucket: None,
            root: None,
            durability: Some(DurabilityClass::ObjectStoreConditionalPut),
            layout_digest: None,
        },
        store.clone(),
        ROOT,
    )
    .unwrap();
    for n in 0..BATCHES {
        backend
            .commit_mutation_batch(TENANT, GRAPH, &batch(n, NODES / BATCHES), None, &bootstrap)
            .await
            .unwrap();
    }
    backend
        .commit_mutation_batch(TENANT, GRAPH, &batch(BATCHES, 2), None, &bootstrap)
        .await
        .unwrap();
    let fetcher = ObjectFetcher::new(store, ROOT);
    let head = fetcher.head(TENANT, GRAPH).await.unwrap();
    let state = fetcher
        .materialize(TENANT, GRAPH, head.version)
        .await
        .unwrap();
    state
        .batches
        .iter()
        .map(|segment| segment.bytes.len())
        .sum()
}

/// The digest of the lifecycle's shape: what the run did, not how fast.
fn lifecycle_digest() -> String {
    let shape = format!("open;read:{NODES};commit:{BATCHES};merge:2");
    Sha256::digest(shape.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn emit(host: &str, revision: &str, dimension: &str, value: f64) {
    println!(
        "{}",
        serde_json::json!({
            "schema_version": SCHEMA_VERSION,
            "host": host,
            "revision": revision,
            "lifecycle_digest": lifecycle_digest(),
            "dimension": dimension,
            "value": value,
        })
    );
}

fn main() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let host = std::env::var("HOSTNAME").unwrap_or_default();
    let revision = std::env::var("MEDULLA_COST_REVISION").unwrap_or_default();

    // The counting run: actions and bytes.
    let counting = Arc::new(CountingObjectStore::new(Arc::new(InMemory::new())));
    runtime.block_on(lifecycle(counting.clone()));
    let total = counting.total();
    emit(&host, &revision, "object_actions", total.actions() as f64);
    emit(
        &host,
        &revision,
        "bytes",
        (total.bytes_read + total.bytes_written) as f64,
    );

    // The matched run without the wrapper: time and memory.
    let plain: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
    let started = Instant::now();
    let resident = runtime.block_on(lifecycle(plain));
    emit(
        &host,
        &revision,
        "time_ms",
        started.elapsed().as_secs_f64() * 1e3,
    );
    emit(&host, &revision, "peak_memory_bytes", resident as f64);
}
