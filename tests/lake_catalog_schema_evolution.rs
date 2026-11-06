//! Iceberg publication wiring, refusal and evidence acceptance
//! (MEDULLA-FEDERATED-QUERY-R079.3, R079.4, R079.5).
//!
//! These tests drive the live `LakeManager` publication path: a series drain
//! publishes a snapshot bound to the committed graph version, every refusal keeps
//! the visible versions unchanged, and each publication, refusal and time-travel
//! read leaves one operation record.

#![cfg(feature = "lake")]

use medulla_lake::catalog::{
    refuse_projection_write, CatalogOperation, IcebergRestCatalog, MANIFEST_PUBLICATION_INVALID,
    PROJECTION_READ_ONLY, SCHEMA_CHANGE_NOT_APPROVED, SCHEMA_COLUMN_UNKNOWN,
};
use medulla_lake::{record_iceberg_commit_bound, LakeField, LakeSchema, LakeTable, LakeType, Lsn};
use medulla_server_blob::blob::store::{BlobManifest, ChunkStore, RedbChunkStore, SweepStats};
use medulla_server_lake::lake::operation_record::OperationOutcome;
use medulla_server_lake::lake::{LakeManager, LakeVisibility, SchemaChange, DEFAULT_NAMESPACE};
use medulla_tsdb::point::Point;
use medulla_tsdb::store::SeriesStore;
use serde_json::Value;
use std::sync::atomic::{AtomicBool, Ordering};

const TEST_BUCKET_NS: u64 = 3_600_000_000_000;

fn unique_dir(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "medulla-lake-schema-evolution-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

fn series_store(tag: &str) -> SeriesStore {
    SeriesStore::open_in_dir(
        &unique_dir(tag),
        medulla_kernel::store_authority::process_verifier(),
        medulla_kernel::store_authority::process_authority().principal(),
        &medulla_kernel::store_authority::process_authority().proof(),
    )
    .expect("open series store")
}

fn points(from: i64, n: i64) -> Vec<Point> {
    (0..n)
        .map(|i| Point::single(from + i, (from + i) as f64 * 1.5))
        .collect()
}

/// A chunk store that refuses writes while armed.
struct FailingStore {
    inner: RedbChunkStore,
    armed: AtomicBool,
}

impl FailingStore {
    fn new() -> Self {
        let dir = unique_dir("chunks");
        FailingStore {
            inner: RedbChunkStore::open(&dir.to_string_lossy()).expect("open chunk store"),
            armed: AtomicBool::new(false),
        }
    }
}

impl ChunkStore for FailingStore {
    fn put_chunk(&self, bytes: &[u8]) -> Result<(String, bool), String> {
        if self.armed.load(Ordering::SeqCst) {
            return Err("injected publication failure".to_string());
        }
        self.inner.put_chunk(bytes)
    }
    fn get_chunk(&self, digest: &str) -> Result<Option<Vec<u8>>, String> {
        self.inner.get_chunk(digest)
    }
    fn put_manifest(&self, blob_digest: &str, manifest: &BlobManifest) -> Result<(), String> {
        self.inner.put_manifest(blob_digest, manifest)
    }
    fn get_manifest(&self, blob_digest: &str) -> Result<Option<BlobManifest>, String> {
        self.inner.get_manifest(blob_digest)
    }
    fn incref(&self, blob_digest: &str) -> Result<u64, String> {
        self.inner.incref(blob_digest)
    }
    fn decref(&self, blob_digest: &str) -> Result<u64, String> {
        self.inner.decref(blob_digest)
    }
    fn refcount(&self, blob_digest: &str) -> Result<u64, String> {
        self.inner.refcount(blob_digest)
    }
    fn sweep(&self) -> Result<SweepStats, String> {
        self.inner.sweep()
    }
    fn chunk_count(&self) -> Result<u64, String> {
        self.inner.chunk_count()
    }
    fn blob_count(&self) -> Result<u64, String> {
        self.inner.blob_count()
    }
}

fn current_snapshot(response: &Value) -> &Value {
    let current_id = &response["metadata"]["current-snapshot-id"];
    response["metadata"]["snapshots"]
        .as_array()
        .expect("Iceberg snapshots array")
        .iter()
        .find(|snapshot| &snapshot["snapshot-id"] == current_id)
        .expect("current-snapshot-id references an emitted snapshot")
}

fn drained(series: &str) -> (FailingStore, SeriesStore, LakeManager) {
    let store = FailingStore::new();
    let tsdb = series_store(series);
    tsdb.append_batch(series, 1, TEST_BUCKET_NS, &["v".to_string()], &points(0, 3))
        .expect("append points");
    let manager = LakeManager::new();
    manager
        .drain_series(&store, &tsdb, series)
        .expect("drain")
        .expect("drain materializes");
    (store, tsdb, manager)
}

// spec: MEDULLA-FEDERATED-QUERY-R079.3
#[test]
fn graph_mutation_reaches_catalog_read() {
    let series = "schema-evolution-read";
    let (_store, _tsdb, manager) = drained(series);
    let loaded = manager
        .load_table(DEFAULT_NAMESPACE, series)
        .expect("catalog read resolves the table");
    let snapshot = current_snapshot(&loaded);
    let version = manager
        .table_lsn(DEFAULT_NAMESPACE, series, &LakeVisibility::Unfiltered)
        .expect("table version");
    assert_eq!(
        snapshot["summary"]["medulla.graph-version"],
        version.to_string()
    );
    assert!(snapshot["summary"]["medulla.source-lsn"].is_string());
    assert!(snapshot["manifest-list"]
        .as_str()
        .is_some_and(|path| path.ends_with("-manifest-list.avro")));

    // A projection CommitTable is refused before any mutation or publication.
    let mut catalog = IcebergRestCatalog::new();
    catalog.register("engine", series, "lake://engine/metadata.json", None);
    let entry = catalog.entry("engine", series).expect("entry").clone();
    let refusal = refuse_projection_write(&CatalogOperation::CommitMetadata, &entry)
        .expect_err("projection commit is refused");
    assert_eq!(refusal.code, PROJECTION_READ_ONLY);
    assert_eq!(
        manager.table_lsn(DEFAULT_NAMESPACE, series, &LakeVisibility::Unfiltered),
        Some(version)
    );
}

// spec: MEDULLA-FEDERATED-QUERY-R079.4
#[test]
fn every_refusal_keeps_visible_versions() {
    let series = "schema-evolution-refusal";
    let (_store, _tsdb, manager) = drained(series);
    let before = manager
        .load_table(DEFAULT_NAMESPACE, series)
        .expect("table");
    let version = manager.table_lsn(DEFAULT_NAMESPACE, series, &LakeVisibility::Unfiltered);

    let unapproved = manager
        .apply_schema_change(
            DEFAULT_NAMESPACE,
            series,
            SchemaChange::AddColumn(LakeField::new("extra", LakeType::Long)),
            false,
        )
        .expect_err("unapproved change is refused");
    assert_eq!(unapproved.code, SCHEMA_CHANGE_NOT_APPROVED);

    let unknown = manager
        .apply_schema_change(
            DEFAULT_NAMESPACE,
            series,
            SchemaChange::DropColumn("no_such_column".to_string()),
            true,
        )
        .expect_err("unknown column is refused");
    assert_eq!(unknown.code, SCHEMA_COLUMN_UNKNOWN);

    let unsupported = manager
        .apply_schema_change(
            DEFAULT_NAMESPACE,
            series,
            SchemaChange::RenameColumn {
                from: "ts".to_string(),
                to: "time".to_string(),
            },
            true,
        )
        .expect_err("unsupported schema feature is refused");
    assert_eq!(unsupported.code, MANIFEST_PUBLICATION_INVALID);
    assert!(!unsupported.reason.is_empty());

    // A snapshot without a graph-version binding is refused and recorded nowhere.
    let schema = LakeSchema::new(vec![LakeField::new("v", LakeType::Double)]);
    let mut table = LakeTable::new("engine", "unbound", schema, "lake://engine/unbound");
    table.record_file("data/a.parquet", 10, 1, Lsn(1));
    let missing = record_iceberg_commit_bound(&mut table, 1, None)
        .expect_err("a missing binding refuses publication");
    assert_eq!(missing.code, MANIFEST_PUBLICATION_INVALID);

    let after = manager
        .load_table(DEFAULT_NAMESPACE, series)
        .expect("table");
    assert_eq!(before, after, "no refusal changed the visible catalog");
    assert_eq!(
        manager.table_lsn(DEFAULT_NAMESPACE, series, &LakeVisibility::Unfiltered),
        version
    );
}

// spec: MEDULLA-FEDERATED-QUERY-R079.5
#[test]
fn evidence_correlates_commit_publication_load() {
    let series = "schema-evolution-evidence";
    let (store, tsdb, manager) = drained(series);
    let records = manager.recent_operations(10);
    let committed = records
        .iter()
        .find(|record| record.outcome == OperationOutcome::Committed)
        .expect("a committed record");
    let loaded = manager
        .load_table(DEFAULT_NAMESPACE, series)
        .expect("table");
    let snapshot = current_snapshot(&loaded);
    assert_eq!(committed.table, format!("{DEFAULT_NAMESPACE}.{series}"));
    assert_eq!(committed.format_version, 2);
    assert_eq!(
        committed.snapshot_id,
        snapshot["snapshot-id"].as_i64(),
        "the record names the published snapshot"
    );
    assert_eq!(
        committed.graph_version.map(|v| v.to_string()).as_deref(),
        snapshot["summary"]["medulla.graph-version"].as_str()
    );
    for digest in [
        &committed.manifest_digest,
        &committed.manifest_list_digest,
        &committed.metadata_digest,
    ] {
        let digest = digest.as_deref().expect("digest recorded");
        assert_eq!(digest.len(), 64);
        assert!(digest
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)));
    }

    // A time-travel read records the snapshot it resolved.
    let lsn = committed.graph_version.expect("graph version");
    manager
        .load_table_as_of(DEFAULT_NAMESPACE, series, lsn, &LakeVisibility::Unfiltered)
        .expect("committed lsn")
        .expect("snapshot exists");
    let read = manager
        .recent_operations(1)
        .pop()
        .expect("time-travel record");
    assert_eq!(read.outcome, OperationOutcome::Read);
    assert_eq!(read.snapshot_id, committed.snapshot_id);

    // A failed publication leaves no success record.
    let committed_before = manager
        .recent_operations(100)
        .iter()
        .filter(|record| record.outcome == OperationOutcome::Committed)
        .count();
    store.armed.store(true, Ordering::SeqCst);
    tsdb.append_batch(series, 1, TEST_BUCKET_NS, &["v".to_string()], &points(3, 3))
        .expect("append more points");
    assert!(manager.drain_series(&store, &tsdb, series).is_err());
    store.armed.store(false, Ordering::SeqCst);
    let records = manager.recent_operations(100);
    let committed_after = records
        .iter()
        .filter(|record| record.outcome == OperationOutcome::Committed)
        .count();
    assert_eq!(committed_before, committed_after);
    let refused = records
        .iter()
        .rev()
        .find(|record| record.outcome == OperationOutcome::Refused)
        .expect("a refusal record");
    assert!(refused.refusal_reason.is_some());
    assert!(refused.snapshot_id.is_none());
}
