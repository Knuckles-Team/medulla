//! The cold open restores the object root into an empty local store
//! (MEDULLA-OBJECT-STORAGE-BACKEND-R002.5).
#![cfg(all(feature = "redb", feature = "object-store", feature = "test-support"))]

use std::sync::Arc;

use futures_util::TryStreamExt;
use medulla_persistence::object_store_backend::{ObjectStoreBackend, OBJECT_ROOT_TENANT};
use medulla_persistence::redb_backend::RedbBackend;
use medulla_protocol_types::protocol::GraphType;
use medulla_server_api::api::persistence::PersistenceBackend;
use medulla_storage::{DurabilityClass, StorageBackendSpec};
use object_store::memory::InMemory;
use object_store::path::Path as ObjPath;
use object_store::ObjectStore;

#[path = "common/replay_fixture.rs"]
mod replay_fixture;
use replay_fixture::{commit_range, GRAPH};

const ROOT: &str = "restore-root";

fn object_backend(store: Arc<InMemory>) -> ObjectStoreBackend {
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
    .expect("the object backend opens")
}

fn local(dir: &std::path::Path, name: &str) -> RedbBackend {
    RedbBackend::open(dir.join(name).to_string_lossy().into_owned(), 64)
        .expect("the local store opens")
}

// spec: MEDULLA-OBJECT-STORAGE-BACKEND-R002.5
/// Verifies MEDULLA-OBJECT-STORAGE-BACKEND-R002.5: three commits published to the root restore
/// into an empty local store, which then holds each published node, and a restore over a root
/// whose segment was corrupted refuses `OBJECT_SEGMENT_DIGEST_MISMATCH`.
#[tokio::test(flavor = "multi_thread")]
async fn restore_materializes_head_into_an_empty_local_store() {
    let _env = medulla_kernel::crypto::acquire_test_env_read_lock().await;
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(InMemory::new());

    let mut source = local(dir.path(), "source");
    source
        .register_graph(GRAPH, GRAPH, GraphType::Global)
        .await
        .unwrap();
    source
        .attach_object_store(Arc::new(object_backend(store.clone())))
        .unwrap();
    commit_range(&source, 0..3).await;
    drop(source);

    // An empty local store restores the head: every published entry applies.
    let fresh = local(dir.path(), "fresh");
    let applied = object_backend(store.clone())
        .restore_into(&fresh)
        .await
        .unwrap();
    assert_eq!(applied, 3);
    for index in 0..3u64 {
        let bytes = fresh
            .read_node(GRAPH, &format!("node-{index}"))
            .await
            .unwrap()
            .unwrap_or_else(|| panic!("node-{index} was restored"));
        let node: serde_json::Value = rmp_serde::from_slice(&bytes).unwrap();
        assert_eq!(node["n"], index);
    }
    // The graph is registered under the name and type its manifest records.
    assert_eq!(
        fresh
            .graph_identity(GRAPH)
            .unwrap()
            .map(|(name, kind, _)| (name, kind)),
        Some((GRAPH.to_string(), GraphType::Global))
    );

    // A segment whose bytes changed no longer matches its digest.
    let prefix = ObjPath::from(format!("{ROOT}/{OBJECT_ROOT_TENANT}/{GRAPH}/segments"));
    let mut keys: Vec<ObjPath> = store
        .list(Some(&prefix))
        .map_ok(|meta| meta.location)
        .try_collect()
        .await
        .unwrap();
    keys.sort();
    let victim = keys.last().cloned().expect("the root holds a segment");
    store
        .put(&victim, b"not the published bytes".to_vec().into())
        .await
        .unwrap();
    let other = local(dir.path(), "corrupt");
    let error = object_backend(store.clone())
        .restore_into(&other)
        .await
        .unwrap_err();
    assert!(
        error.starts_with("OBJECT_SEGMENT_DIGEST_MISMATCH"),
        "{error}"
    );
}
