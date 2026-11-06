//! Iceberg manifest publication acceptance (MEDULLA-FEDERATED-QUERY-R079.2.1).
//!
//! The first test drives the table's snapshot log through append, compaction and
//! copy-on-write deletion, builds the manifests of every committed snapshot and
//! decodes each artifact, asserting counts, sizes and entry statuses. The second
//! half drives the live `LakeManager` publication path with an object store that
//! fails before the metadata artifact lands, and asserts the previous snapshot
//! stays readable.

#![cfg(feature = "lake")]

use medulla_lake::iceberg_avro::{read_manifest_entries, read_manifest_list};
use medulla_lake::{LakeField, LakeSchema, LakeTable, LakeType, Lsn};
use medulla_server_blob::blob::store::{BlobManifest, ChunkStore, RedbChunkStore, SweepStats};
use medulla_server_lake::lake::{LakeManager, DEFAULT_NAMESPACE};
use medulla_tsdb::point::Point;
use medulla_tsdb::store::SeriesStore;
use std::sync::atomic::{AtomicBool, Ordering};

const TEST_BUCKET_NS: u64 = 3_600_000_000_000;
const LOCATION: &str = "lake://engine/interop";

/// One manifest entry: `(status, path, rows, bytes)`.
type ManifestEntry = (i32, String, i64, i64);

/// Decoded view of one committed snapshot: `(status, path, rows, bytes)` per entry.
fn decode(table: &LakeTable) -> (Vec<ManifestEntry>, [i32; 3], [i64; 3]) {
    let manifests = table.iceberg_manifests().expect("manifests build");
    let entries = read_manifest_entries(&manifests.manifest_avro).expect("manifest decodes");
    let lists = read_manifest_list(&manifests.manifest_list_avro).expect("manifest list decodes");
    assert_eq!(lists.len(), 1, "one manifest per snapshot");
    assert_eq!(lists[0].manifest_path, manifests.manifest_path);
    assert_eq!(
        lists[0].manifest_length,
        manifests.manifest_avro.len() as i64
    );
    assert_eq!(lists[0].added_snapshot_id, manifests.snapshot_id);
    let view = entries
        .into_iter()
        .map(|e| (e.status, e.file_path, e.record_count, e.file_size_in_bytes))
        .collect();
    (view, lists[0].file_counts, lists[0].row_counts)
}

fn path(name: &str) -> String {
    format!("{LOCATION}/data/{name}.parquet")
}

// spec: MEDULLA-FEDERATED-QUERY-R079.2.1
#[test]
fn append_compaction_cow_delete() {
    let schema = LakeSchema::new(vec![LakeField::new("v", LakeType::Long)]);
    let mut table = LakeTable::new("engine", "interop", schema, LOCATION);

    // Snapshot 1: the first append.
    table.record_file("data/a.parquet", 100, 3, Lsn(1));
    let (entries, files, rows) = decode(&table);
    assert_eq!(entries, vec![(1, path("a"), 3, 100)]);
    assert_eq!((files, rows), ([1, 0, 0], [3, 0, 0]));

    // Snapshot 2: an append keeps the first file EXISTING.
    table.record_file("data/b.parquet", 200, 2, Lsn(2));
    let (mut entries, files, rows) = decode(&table);
    entries.sort();
    assert_eq!(
        entries,
        vec![(0, path("a"), 3, 100), (1, path("b"), 2, 200)]
    );
    assert_eq!((files, rows), ([1, 1, 0], [2, 3, 0]));

    // Snapshot 3: compaction retires both files and adds one.
    table.snapshot.remove_file("data/a.parquet", Lsn(3));
    table.snapshot.remove_file("data/b.parquet", Lsn(3));
    table.record_file("data/c.parquet", 250, 5, Lsn(3));
    let (mut entries, files, rows) = decode(&table);
    entries.sort();
    assert_eq!(
        entries,
        vec![
            (1, path("c"), 5, 250),
            (2, path("a"), 3, 100),
            (2, path("b"), 2, 200),
        ]
    );
    assert_eq!((files, rows), ([1, 0, 2], [5, 0, 5]));

    // Snapshot 4: copy-on-write deletion rewrites the file without two rows.
    table.snapshot.remove_file("data/c.parquet", Lsn(4));
    table.record_file("data/d.parquet", 160, 3, Lsn(4));
    let (mut entries, files, rows) = decode(&table);
    entries.sort();
    assert_eq!(
        entries,
        vec![(1, path("d"), 3, 160), (2, path("c"), 5, 250)]
    );
    assert_eq!((files, rows), ([1, 0, 1], [3, 0, 5]));

    // Snapshot 5: an append after the rewrite lists the retired files no more.
    table.record_file("data/e.parquet", 90, 1, Lsn(5));
    let (mut entries, files, rows) = decode(&table);
    entries.sort();
    assert_eq!(entries, vec![(0, path("d"), 3, 160), (1, path("e"), 1, 90)]);
    assert_eq!((files, rows), ([1, 1, 0], [1, 3, 0]));
}

/// A chunk store that refuses writes while armed, standing in for a failure
/// before the metadata artifact is published.
struct FailingStore {
    inner: RedbChunkStore,
    armed: AtomicBool,
}

impl ChunkStore for FailingStore {
    fn put_chunk(&self, bytes: &[u8]) -> Result<(String, bool), String> {
        if self.armed.load(Ordering::SeqCst) {
            return Err("injected failure before metadata publication".to_string());
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

fn open_chunk_store() -> RedbChunkStore {
    let dir = std::env::temp_dir().join(format!(
        "medulla-lake-interop-chunks-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).expect("create chunk store dir");
    RedbChunkStore::open(&dir.to_string_lossy()).expect("open chunk store")
}

fn series_store(tag: &str) -> SeriesStore {
    let dir = std::env::temp_dir().join(format!(
        "medulla-lake-interop-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).expect("create tsdb dir");
    SeriesStore::open_in_dir(
        &dir,
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

// spec: MEDULLA-FEDERATED-QUERY-R079.2.1
#[test]
fn failure_before_metadata_publication_keeps_previous_snapshot_readable() {
    let store = FailingStore {
        inner: open_chunk_store(),
        armed: AtomicBool::new(false),
    };
    let tsdb = series_store("failure");
    let series = "interop-failure";
    tsdb.append_batch(series, 1, TEST_BUCKET_NS, &["v".to_string()], &points(0, 3))
        .expect("append points");
    let manager = LakeManager::new();
    manager
        .drain_series(&store, &tsdb, series)
        .expect("first drain")
        .expect("first drain materializes");
    let before = manager
        .load_table(DEFAULT_NAMESPACE, series)
        .expect("table is registered");

    store.armed.store(true, Ordering::SeqCst);
    tsdb.append_batch(series, 1, TEST_BUCKET_NS, &["v".to_string()], &points(3, 3))
        .expect("append more points");
    let failed = manager.drain_series(&store, &tsdb, series);
    assert!(
        failed.is_err(),
        "the injected failure refuses the publication"
    );
    store.armed.store(false, Ordering::SeqCst);

    let after = manager
        .load_table(DEFAULT_NAMESPACE, series)
        .expect("previous snapshot stays readable");
    assert_eq!(before, after, "no partial snapshot became visible");
}
