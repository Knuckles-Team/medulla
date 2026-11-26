//! The archiver copies committed batch records into ordered archive segments
//! and resumes from the manifest (MEDULLA-ENGINE-PARITY-R016.1).

use super::*;
use medulla_storage::object::archive::ObjectArchive;
use medulla_storage::{
    archive_store_pass, decode_batch_record, ledger_scope_key, ArchivePolicy, ArchiveSegment,
};

const TENANT: &str = "tenant-a";

fn commit_range(
    fixture: &Fixture,
    owner: &OwnedStoreHandle<LedgerOnlyOwner>,
    identity: &MutationScopeIdentity,
    range: std::ops::Range<u64>,
) {
    for index in range {
        let mut next = batch(identity.clone(), &format!("b{index}"));
        next.version_expectation = VersionExpectation::Native(index);
        apply_batch(fixture, owner, &next);
    }
}

/// Every manifest entry of `graph`, as `(first, last)` plus the decoded batch ids
/// of the records its segment holds.
async fn archived(archive: &ObjectArchive, graph: &str) -> Vec<(u64, u64, Vec<String>)> {
    let mut out = Vec::new();
    for entry in archive.segments(TENANT, graph).await.unwrap() {
        let bytes = archive
            .get_segment(TENANT, graph, entry.first, entry.last)
            .await
            .unwrap();
        let segment: ArchiveSegment = rmp_serde::from_slice(&bytes).unwrap();
        assert_eq!(segment.scope_key, graph);
        let ids = segment
            .records
            .iter()
            .map(|stored| decode_batch_record(stored).unwrap().batch.batch_id)
            .collect();
        out.push((entry.first, entry.last, ids));
    }
    out
}

// spec: MEDULLA-ENGINE-PARITY-R016.1
#[tokio::test]
async fn archive_pass_writes_ordered_segments_and_resumes() {
    let dir = tempfile::tempdir().unwrap();
    let identity = native_identity("tenant-a", "incarnation:blob:archive");
    let (fixture, owner) = ledger_fixture(&dir.path().join("native.redb"), identity.clone());
    let graph = ledger_scope_key(&identity);
    let archive = ObjectArchive::new(Arc::new(object_store::memory::InMemory::new()), "root");
    let policy = ArchivePolicy {
        segment_bytes: 4 * 1024,
        retain_s: 3_600,
        interval_s: 10,
    };

    commit_range(&fixture, &owner, &identity, 0..50);
    let report = archive_store_pass(&fixture.kernel, &archive, TENANT, &policy)
        .await
        .unwrap();
    assert_eq!(report.records_archived, 50);
    assert!(report.segments_written > 1, "a small bound splits the scope");

    let first_pass = archived(&archive, &graph).await;
    assert_eq!(first_pass.len() as u64, report.segments_written);
    let mut expected_first = 1;
    for (first, last, ids) in &first_pass {
        assert_eq!(*first, expected_first, "segments are contiguous");
        assert_eq!(*last - *first, ids.len() as u64);
        for (offset, id) in ids.iter().enumerate() {
            assert_eq!(id, &format!("b{}", first - 1 + offset as u64));
        }
        expected_first = *last;
    }
    assert_eq!(expected_first, 51, "every committed sequence is covered");

    commit_range(&fixture, &owner, &identity, 50..60);
    let report = archive_store_pass(&fixture.kernel, &archive, TENANT, &policy)
        .await
        .unwrap();
    assert_eq!(report.records_archived, 10, "only the new range is written");
    let second_pass = archived(&archive, &graph).await;
    assert_eq!(second_pass[..first_pass.len()], first_pass[..]);
    assert_eq!(second_pass.first().map(|entry| entry.0), Some(1));
    assert_eq!(second_pass.last().map(|entry| entry.1), Some(61));
    let again = archive_store_pass(&fixture.kernel, &archive, TENANT, &policy)
        .await
        .unwrap();
    assert_eq!(again.segments_written, 0);
}

// spec: MEDULLA-ENGINE-PARITY-R016.4
/// A cut-off after every segment expires all but the newest, from the manifest and the
/// store alike.
#[tokio::test]
async fn retention_expires_old_segments_and_keeps_the_newest() {
    let dir = tempfile::tempdir().unwrap();
    let identity = native_identity("tenant-a", "incarnation:blob:retention");
    let (fixture, owner) = ledger_fixture(&dir.path().join("native.redb"), identity.clone());
    let graph = ledger_scope_key(&identity);
    let archive = ObjectArchive::new(Arc::new(object_store::memory::InMemory::new()), "root");
    // One byte per segment: every committed record forms its own segment.
    let policy = ArchivePolicy {
        segment_bytes: 1,
        retain_s: 3_600,
        interval_s: 10,
    };
    commit_range(&fixture, &owner, &identity, 0..3);
    let report = archive_store_pass(&fixture.kernel, &archive, TENANT, &policy)
        .await
        .unwrap();
    assert_eq!(report.segments_written, 3);

    // The manifest stamps each segment with the commit time of its last record.
    let entries = archive.segments(TENANT, &graph).await.unwrap();
    assert_eq!(entries.len(), 3);
    for entry in &entries {
        let bytes = archive
            .get_segment(TENANT, &graph, entry.first, entry.last)
            .await
            .unwrap();
        let segment: ArchiveSegment = rmp_serde::from_slice(&bytes).unwrap();
        let last = decode_batch_record(segment.records.last().unwrap()).unwrap();
        assert_eq!(entry.last_committed_at_ms, last.committed_at_ms);
    }

    // A cut-off after all three expires the first two and keeps the newest.
    let expired = archive
        .expire_segments(TENANT, &graph, u64::MAX)
        .await
        .unwrap();
    assert_eq!(expired, 2);
    let kept = archive.segments(TENANT, &graph).await.unwrap();
    assert_eq!(kept, vec![entries[2].clone()]);
    for gone in &entries[..2] {
        assert!(archive
            .get_segment(TENANT, &graph, gone.first, gone.last)
            .await
            .is_err());
    }
    assert!(archive
        .get_segment(TENANT, &graph, entries[2].first, entries[2].last)
        .await
        .is_ok());
    assert_eq!(archive.archived_end(TENANT, &graph).await.unwrap(), 4);
}
