//! The index owns the identity the compaction worker binds into the low-storage
//! manifest, and it survives a reopen of the store in process
//! (MEDULLA-FEDERATED-QUERY-R040.5.6).
#![cfg(not(feature = "ann"))]

use medulla_ann::low_storage::{LowStorageArtifact, LowStorageErrorCode};
use medulla_index_types::embedding::EmbeddingSpaceRef;
use medulla_semantic_index::index_identity::IndexIdentity;
use medulla_semantic_index::semantic::SemanticStore;

const DIM: usize = 8;
const ROWS: usize = 64;

fn vector(i: usize) -> Vec<f32> {
    (0..DIM)
        .map(|d| ((i * 31 + d * 7 + 1) % 17) as f32 + 1.0)
        .collect()
}

fn identity() -> IndexIdentity {
    IndexIdentity {
        tenant: "tenant-1".into(),
        snapshot: "snap-1".into(),
        source_id: "src-1".into(),
        revision: 3,
        text_digest: [9u8; 32],
        encoding_policy: "sq8".into(),
        build_policy: "build-1".into(),
        generation: 5,
        ceiling_bytes: 1 << 30,
    }
}

fn built_store() -> SemanticStore {
    let mut store = SemanticStore::new();
    store
        .declare_space(EmbeddingSpaceRef::pinned("m", "1", "mh", "ph", DIM, true).unwrap())
        .unwrap();
    for i in 0..ROWS {
        store.add_embedding(format!("n{i}"), vector(i)).unwrap();
    }
    store.declare_identity(identity()).unwrap();
    store
}

// spec: MEDULLA-FEDERATED-QUERY-R040.5.6
#[test]
fn compaction_worker_binds_the_identity_the_index_persisted_across_a_reopen() {
    let mut store = built_store();
    assert!(!store.semantic_search(&vector(1), 3).is_empty());
    // A search builds the resident index; an overwrite then leaves a tombstone; the worker drops it and binds the manifest.
    store.add_embedding("n3".into(), vector(500)).unwrap();
    assert!(!store.ann_compacted());
    let artifact = store.compact_low_storage().unwrap();
    assert!(store.ann_compacted());
    let id = identity();
    let space_digest = store.space().unwrap().digest.clone();
    assert_eq!(artifact.manifest.tenant, id.tenant);
    assert_eq!(artifact.manifest.snapshot, id.snapshot);
    assert_eq!(artifact.manifest.source_id, id.source_id);
    assert_eq!(artifact.manifest.revision, id.revision);
    assert_eq!(artifact.manifest.text_digest, id.text_digest);
    assert_eq!(artifact.manifest.encoding_policy, id.encoding_policy);
    assert_eq!(artifact.manifest.build_policy, id.build_policy);
    assert_eq!(artifact.manifest.generation, id.generation);
    assert_eq!(artifact.manifest.embedding_space, space_digest);
    assert_eq!(artifact.ids().len(), ROWS);
    assert!(artifact.tombstones().is_empty());
    assert!(store.low_storage_artifact().is_some());

    // A write clears the resident artifact.
    store.add_embedding("n4".into(), vector(501)).unwrap();
    assert!(store.low_storage_artifact().is_none());

    // Reopen in process: serialise the store, drop it and read it back.
    let bytes = serde_json::to_vec(&store).unwrap();
    drop(store);
    let reopened: SemanticStore = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(reopened.identity(), Some(&id));
    let again = reopened.compact_low_storage().unwrap();
    assert_eq!(again.manifest.tenant, id.tenant);
    assert_eq!(again.manifest.text_digest, id.text_digest);
    assert_eq!(again.manifest.embedding_space, space_digest);
    assert_eq!(again.ids().len(), ROWS);

    // The artifact decodes only under the generation its manifest names.
    let image = again.encode().unwrap();
    let back = LowStorageArtifact::decode(&image, id.generation).unwrap();
    assert_eq!(back.manifest, again.manifest);
    assert_eq!(
        LowStorageArtifact::decode(&image, id.generation + 1)
            .unwrap_err()
            .code,
        LowStorageErrorCode::ArtifactInvalid
    );
}

// spec: MEDULLA-FEDERATED-QUERY-R040.5.6
#[test]
fn an_index_without_an_identity_refuses_and_a_different_identity_is_refused() {
    let mut store = SemanticStore::new();
    store
        .declare_space(EmbeddingSpaceRef::pinned("m", "1", "mh", "ph", DIM, true).unwrap())
        .unwrap();
    for i in 0..ROWS {
        store.add_embedding(format!("n{i}"), vector(i)).unwrap();
    }
    assert_eq!(
        store.compact_low_storage().unwrap_err().code,
        LowStorageErrorCode::ManifestInvalid
    );
    store.declare_identity(identity()).unwrap();
    let mut other = identity();
    other.tenant = "tenant-2".into();
    assert!(store.declare_identity(other).is_err());
    assert!(store.compact_low_storage().is_ok());
}
