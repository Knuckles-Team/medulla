//! Object-read bound test for MEDULLA-OBJECT-STORAGE-BACKEND-R002:
//! versioned reads against the in-memory object store materialize each
//! version's own write set, a corrupted segment refuses with a typed error
//! naming the version and key, a manifest naming a missing segment refuses
//! typed, and earlier versions stay readable.

#![cfg(feature = "object-archive")]

use medulla_storage::object::fetch::{
    ObjectFetcher, ObjectReadRefusal, SegmentHeader, VersionManifest,
};
use object_store::memory::InMemory;
use object_store::{ObjectStore, ObjectStoreExt};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::sync::Arc;

fn hex_digest(bytes: &[u8]) -> String {
    let out = Sha256::digest(bytes).to_vec();
    out.iter().map(|b| format!("{b:02x}")).collect()
}

struct VersionWriteSet {
    version: u64,
    parent_version: Option<u64>,
    snapshot_bytes: Vec<u8>,
    batches: Vec<Vec<u8>>,
}

impl VersionWriteSet {
    fn manifest(&self) -> VersionManifest {
        let seg = |bytes: &[u8]| SegmentHeader {
            key: hex_digest(bytes),
            sha256: hex_digest(bytes),
        };
        let mut manifest = VersionManifest {
            schema_version: 1,
            tenant: "tenant-a".to_string(),
            graph: "g1".to_string(),
            version: self.version,
            parent_version: self.parent_version,
            snapshot: seg(&self.snapshot_bytes),
            batches: self.batches.iter().map(|b| seg(b)).collect(),
            digest: String::new(),
        };
        manifest.digest = manifest.body_digest().expect("body digest");
        manifest
    }

    /// Where the fetcher reads a nested batch segment not exposed as one.
    fn exposes(&self) -> impl Iterator<Item = &Vec<u8>> {
        self.batches
            .iter()
            .chain(std::iter::once(&self.snapshot_bytes))
    }
}

// spec: MEDULLA-OBJECT-STORAGE-BACKEND-R002
#[tokio::test]
async fn versioned_reads_and_corruption_refusal() {
    let shared: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
    let sets = vec![
        VersionWriteSet {
            version: 1,
            parent_version: None,
            snapshot_bytes: b"snapshot-v1-owner-tables".to_vec(),
            batches: vec![b"batch-a".to_vec(), b"batch-b".to_vec()],
        },
        VersionWriteSet {
            version: 2,
            parent_version: Some(1),
            snapshot_bytes: b"snapshot-v2-owner-tables".to_vec(),
            batches: vec![b"batch-c".to_vec()],
        },
        VersionWriteSet {
            version: 3,
            parent_version: Some(2),
            snapshot_bytes: b"snapshot-v3-owner-tables".to_vec(),
            batches: vec![],
        },
    ];
    for ws in &sets {
        let manifest = ws.manifest();
        shared
            .put(
                &object_store::path::Path::from(format!(
                    "root/tenant_a/g1/manifests/{:020}.json",
                    ws.version
                )),
                serde_json::to_vec(&manifest).unwrap().into(),
            )
            .await
            .expect("manifest write");
        for bytes in ws.exposes() {
            shared
                .put(
                    &object_store::path::Path::from(format!(
                        "root/tenant_a/g1/segments/{}.seg",
                        hex_digest(bytes)
                    )),
                    bytes.clone().into(),
                )
                .await
                .expect("segment write");
        }
    }
    shared
        .put(
            &object_store::path::Path::from("root/tenant_a/g1/HEAD"),
            json!({"version": 3, "manifest": "manifests/00000000000000000003.json"})
                .to_string()
                .into_bytes()
                .into(),
        )
        .await
        .expect("head write");

    let fetcher = ObjectFetcher::new(shared.clone(), "root");

    // HEAD points at the newest version.
    let head = fetcher.head("tenant-a", "g1").await.expect("head");
    assert_eq!(head.version, 3);

    // Each version materializes its own write set.
    for ws in &sets {
        let got = fetcher
            .materialize("tenant-a", "g1", ws.version)
            .await
            .expect("materialize");
        assert_eq!(got.version, ws.version);
        assert_eq!(got.parent_version, ws.parent_version);
        assert_eq!(got.snapshot.bytes, ws.snapshot_bytes);
        let got_batches: Vec<Vec<u8>> = got.batches.iter().map(|s| s.bytes.clone()).collect();
        assert_eq!(got_batches, ws.batches);
        // Every segment is content-addressed: its bytes hash to its key.
        for seg in got.batches.iter() {
            assert_eq!(seg.sha256, seg.key);
        }
    }

    // A corrupted segment refuses as exact typed code, version and key named.
    let corrupt_key = hex_digest(&sets[2].snapshot_bytes);
    shared
        .put(
            &object_store::path::Path::from(format!("root/tenant_a/g1/segments/{corrupt_key}.seg")),
            b"tampered".to_vec().into(),
        )
        .await
        .expect("corruption write");
    match fetcher.materialize("tenant-a", "g1", 3).await {
        Err(ObjectReadRefusal::DigestMismatch { version, key }) => {
            assert_eq!(version, 3);
            assert_eq!(key, corrupt_key);
            assert_eq!(
                "OBJECT_SEGMENT_DIGEST_MISMATCH",
                ObjectReadRefusal::DigestMismatch { version: 3, key }.wire_code()
            );
        }
        other => panic!("expected DigestMismatch, got {other:?}"),
    }

    // A manifest that names a missing segment refuses with a typed error.
    let ghost = vec![0u8; 32];
    let ghost_key = hex_digest(&ghost);
    let mut version4 = VersionWriteSet {
        version: 4,
        parent_version: Some(3),
        snapshot_bytes: ghost,
        batches: Vec::new(),
    };
    version4.snapshot_bytes = vec![9u8; 8];
    // Point the snapshot header at a key whose object was never written.
    let mut manifest4 = version4.manifest();
    manifest4.snapshot = SegmentHeader {
        key: ghost_key.clone(),
        sha256: ghost_key.clone(),
    };
    manifest4.digest = manifest4.body_digest().expect("body digest");
    shared
        .put(
            &object_store::path::Path::from("root/tenant_a/g1/manifests/00000000000000000004.json"),
            serde_json::to_vec(&manifest4).unwrap().into(),
        )
        .await
        .expect("orphan manifest write");
    match fetcher.materialize("tenant-a", "g1", 4).await {
        Err(ObjectReadRefusal::SegmentMissing { version, key }) => {
            assert_eq!(version, 4);
            assert_eq!(key, ghost_key);
            assert_eq!(
                "OBJECT_SEGMENT_MISSING",
                ObjectReadRefusal::SegmentMissing { version: 4, key }.wire_code()
            );
        }
        other => panic!("expected SegmentMissing, got {other:?}"),
    }

    // The versions the corrupted leaf cannot touch stay intact and readable.
    for ws in &sets[..2] {
        let got = fetcher
            .materialize("tenant-a", "g1", ws.version)
            .await
            .expect("earlier versions stay intact");
        assert_eq!(got.version, ws.version);
        assert_eq!(got.snapshot.bytes, ws.snapshot_bytes);
    }
}
