//! Iceberg REST projection side (MEDULLA-FEDERATED-QUERY-R080.6): over a real TCP listener a
//! client write never changes a lake table, and the engine's own publication does.
//!
//! The test builds a `RedbChunkStore`, a `SeriesStore` and a `LakeManager` the way
//! `tests/adopt_lake_asof_lsn_gap.rs` does, publishes a time series with
//! `LakeManager::drain_series`, binds a loopback listener and serves the catalog with
//! `medulla_server_lake::lake::rest::serve`. Over plain HTTP it loads the table, refuses a data
//! commit and a drop with `405 PROJECTION_READ_ONLY`, loads again and finds the snapshot and the
//! LSN unchanged, then publishes two more points and finds a new snapshot. No handler is called
//! directly and no catalog response is mocked.

#![cfg(all(feature = "lake", feature = "lake-rest"))]

use std::sync::Arc;

use medulla_server_blob::blob::store::{ChunkStore, RedbChunkStore};
use medulla_server_lake::lake::{rest, LakeManager, LakeVisibility, DEFAULT_NAMESPACE};
use medulla_tsdb::point::Point;
use medulla_tsdb::store::SeriesStore;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

const TEST_BUCKET_NS: u64 = 3_600_000_000_000;
const SERIES: &str = "projection.s1";
const TABLE: &str = "projection_s1";

fn unique_dir(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "medulla-lake-projection-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

fn points(from: i64, n: i64) -> Vec<Point> {
    (0..n)
        .map(|i| Point::single(from + i, (from + i) as f64 * 1.5))
        .collect()
}

/// One HTTP/1.1 request over a fresh connection; answers the status code and the body.
async fn http(addr: std::net::SocketAddr, method: &str, path: &str, body: &str) -> (u16, String) {
    let mut stream = TcpStream::connect(addr)
        .await
        .expect("connect to the catalog");
    let request = format!(
        "{method} {path} HTTP/1.1\r\nhost: localhost\r\ncontent-type: application/json\r\n\
         content-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
    stream
        .write_all(request.as_bytes())
        .await
        .expect("send the request");
    let mut raw = Vec::new();
    stream
        .read_to_end(&mut raw)
        .await
        .expect("read the response");
    let text = String::from_utf8_lossy(&raw).into_owned();
    let status = text
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse().ok())
        .expect("an HTTP status line");
    let body = text
        .split_once("\r\n\r\n")
        .map(|(_, body)| body.to_string())
        .unwrap_or_default();
    (status, body)
}

/// `metadata.current-snapshot-id` of a loaded table.
async fn snapshot_id(addr: std::net::SocketAddr) -> u64 {
    let path = format!("/v1/namespaces/{DEFAULT_NAMESPACE}/tables/{TABLE}");
    let (status, body) = http(addr, "GET", &path, "").await;
    assert_eq!(status, 200, "{body}");
    let loaded: serde_json::Value = serde_json::from_str(&body).expect("a JSON table");
    loaded["metadata"]["current-snapshot-id"]
        .as_u64()
        .expect("a current snapshot id")
}

// spec: MEDULLA-FEDERATED-QUERY-R080.6
#[tokio::test(flavor = "multi_thread")]
async fn projection_writes_are_refused_and_graph_writes_appear() {
    let chunks = unique_dir("chunks");
    let store: Arc<dyn ChunkStore> =
        Arc::new(RedbChunkStore::open(&chunks.to_string_lossy()).expect("open the chunk store"));
    let tsdb = SeriesStore::open_in_dir(
        &unique_dir("series"),
        medulla_kernel::store_authority::process_verifier(),
        medulla_kernel::store_authority::process_authority().principal(),
        &medulla_kernel::store_authority::process_authority().proof(),
    )
    .expect("open the series store");
    let lake = Arc::new(LakeManager::new());

    tsdb.append_batch(SERIES, 1, TEST_BUCKET_NS, &["v".to_string()], &points(0, 3))
        .expect("append the first points");
    lake.drain_series(store.as_ref(), &tsdb, SERIES)
        .expect("the first drain")
        .expect("the first drain publishes");

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind a loopback listener");
    let addr = listener.local_addr().expect("the listener address");
    tokio::spawn(rest::serve(listener, lake.clone(), store.clone()));

    // (1) The table is readable; record its snapshot and LSN.
    let first = snapshot_id(addr).await;
    let lsn = lake
        .table_lsn(DEFAULT_NAMESPACE, TABLE, &LakeVisibility::Unfiltered)
        .expect("the table has an LSN");

    // (2) A data commit and a drop are refused as a read-only projection.
    let path = format!("/v1/namespaces/{DEFAULT_NAMESPACE}/tables/{TABLE}");
    let (status, body) = http(addr, "POST", &path, r#"{"requirements":[],"updates":[]}"#).await;
    assert_eq!(status, 405, "{body}");
    assert!(body.contains("PROJECTION_READ_ONLY"), "{body}");
    assert!(body.contains(TABLE), "{body}");
    let (status, body) = http(addr, "DELETE", &path, "").await;
    assert_eq!(status, 405, "{body}");
    assert!(body.contains("PROJECTION_READ_ONLY"), "{body}");

    // (3) Neither refusal moved the snapshot or the LSN.
    assert_eq!(snapshot_id(addr).await, first);
    assert_eq!(
        lake.table_lsn(DEFAULT_NAMESPACE, TABLE, &LakeVisibility::Unfiltered),
        Some(lsn)
    );

    // (4) The engine's own publication is visible to the reader: a new snapshot.
    tsdb.append_batch(SERIES, 1, TEST_BUCKET_NS, &["v".to_string()], &points(3, 2))
        .expect("append the next points");
    lake.drain_series(store.as_ref(), &tsdb, SERIES)
        .expect("the second drain")
        .expect("the second drain publishes");
    let second = snapshot_id(addr).await;
    assert_ne!(second, first, "the engine publication is a new snapshot");
}
