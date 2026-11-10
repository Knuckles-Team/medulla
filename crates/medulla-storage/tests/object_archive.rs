//! Archive-segment bound test for MEDULLA-OBJECT-STORAGE-BACKEND-R006:
//! an overlapping range refuses with `ARCHIVE_RANGE_OVERLAP` and an altered
//! segment refuses with `ARCHIVE_DIGEST_MISMATCH`, both against the in-memory
//! object store (no S3 required).

#![cfg(feature = "object-archive")]

use medulla_storage::object::archive::{ArchiveRefusal, ObjectArchive};
use object_store::memory::InMemory;
use object_store::{ObjectStore, ObjectStoreExt};
use std::sync::Arc;

fn archive() -> ObjectArchive {
    ObjectArchive::new(Arc::new(InMemory::new()), "root")
}

// spec: MEDULLA-OBJECT-STORAGE-BACKEND-R006
#[tokio::test]
async fn overlap_refuses_and_adjacent_range_succeeds() {
    let arch = archive();
    let first = b"segment-zero".to_vec();
    let key = arch
        .put_segment("tenant-a", "g1", 0, 10, &first)
        .await
        .expect("first put succeeds");
    assert_eq!(key, "00000000000000000000-00000000000000000010.seg");

    // Overlapping inside: refuses before any write.
    match arch.put_segment("tenant-a", "g1", 5, 15, b"other").await {
        Err(ArchiveRefusal::RangeOverlap { first, last }) => {
            assert_eq!((first, last), (5, 15));
        }
        other => panic!("expected RangeOverlap, got {other:?}"),
    }
    assert_eq!(
        arch.put_segment("tenant-a", "g1", 5, 15, b"other")
            .await
            .err()
            .unwrap()
            .wire_code(),
        "ARCHIVE_RANGE_OVERLAP"
    );
    // Exact re-put of an already archived range is an overlap too: immutable.
    match arch
        .put_segment("tenant-a", "g1", 0, 10, b"segment-zero")
        .await
    {
        Err(ArchiveRefusal::RangeOverlap { .. }) => {}
        other => panic!("expected RangeOverlap for re-put, got {other:?}"),
    }
    // Adjacent (half-open touch) does not overlap and succeeds.
    arch.put_segment("tenant-a", "g1", 10, 20, b"segment-one")
        .await
        .expect("adjacent put succeeds");
    assert_eq!(
        arch.get_segment("tenant-a", "g1", 0, 10).await.unwrap(),
        first
    );
}

// spec: MEDULLA-OBJECT-STORAGE-BACKEND-R006
#[tokio::test]
async fn altered_segment_refuses_digest_mismatch() {
    let shared: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
    let arch = ObjectArchive::new(shared.clone(), "root");
    arch.put_segment("tenant-a", "g1", 0, 10, b"original-bytes")
        .await
        .expect("put succeeds");
    // Alter the stored object underneath the manifest digest, as a corrupted
    // store would.
    let seg_path = object_store::path::Path::from(
        "root/archive/tenant_a/g1/00000000000000000000-00000000000000000010.seg",
    );
    shared
        .put(&seg_path, b"tampered-bytes".to_vec().into())
        .await
        .expect("tamper write");
    match arch.get_segment("tenant-a", "g1", 0, 10).await {
        Err(ArchiveRefusal::DigestMismatch { first, last }) => {
            assert_eq!((first, last), (0, 10));
            assert_eq!(
                "ARCHIVE_DIGEST_MISMATCH",
                ArchiveRefusal::DigestMismatch { first: 0, last: 10 }.wire_code()
            );
        }
        other => panic!("expected DigestMismatch, got {other:?}"),
    }
}

// spec: MEDULLA-OBJECT-STORAGE-BACKEND-R006
#[tokio::test]
async fn missing_range_refuses() {
    let arch = archive();
    match arch.get_segment("tenant-a", "g1", 100, 200).await {
        Err(ArchiveRefusal::SegmentMissing { first, last }) => {
            assert_eq!((first, last), (100, 200));
        }
        other => panic!("expected SegmentMissing, got {other:?}"),
    }
}
