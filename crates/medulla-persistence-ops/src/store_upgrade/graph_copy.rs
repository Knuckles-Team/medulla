//! Offline copy of one graph from the redb store to an object-store root with a verified
//! digest (MEDULLA-OBJECT-STORAGE-BACKEND-R003.2).
//!
//! `copy_graph` streams the graph's rows out of the stopped engine's store in pages of
//! 1,000 rows through `RedbBackend::export_graph_raw_page`, so memory holds one page at a
//! time. Each page is one segment with a row count and a SHA-256 digest. Segments are
//! staged under a `staging/` prefix of the graph; the whole-graph digest, a chain over the
//! segment digests and row counts, is computed from the source and again from what the
//! destination returns on read-back. Only when the two match are the segments published
//! under their digests, followed by the version manifest and `HEAD`. A mismatch removes the
//! staged segments and leaves the destination empty. A destination that already holds
//! anything under the graph is refused before a byte is written.

use std::sync::Arc;

use futures_util::TryStreamExt;
use medulla_persistence::redb_backend::RedbBackend;
use medulla_storage::object::fetch::{ObjectHead, SegmentHeader, VersionManifest};
use object_store::path::Path as ObjPath;
use object_store::{ObjectStore, ObjectStoreExt, PutMode, PutOptions};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// The wire code of a copy whose destination digest differs from the source digest.
pub const GRAPH_COPY_DIGEST_MISMATCH: &str = "GRAPH_COPY_DIGEST_MISMATCH";
/// The wire code of a copy refused because the destination graph is not empty.
pub const GRAPH_COPY_DESTINATION_NOT_EMPTY: &str = "GRAPH_COPY_DESTINATION_NOT_EMPTY";
/// Rows of one batch.
pub const BATCH_ROWS: usize = 1_000;
/// SHA-256 of the empty snapshot segment every copied graph starts from (version 0).
const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

/// One batch segment: a page of node and edge rows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RowBatch {
    pub nodes: Vec<(String, Vec<u8>)>,
    pub edges: Vec<(String, String, Vec<u8>)>,
}

impl RowBatch {
    fn rows(&self) -> u64 {
        (self.nodes.len() + self.edges.len()) as u64
    }
}

/// Why a copy did not happen.
#[derive(Debug, PartialEq, Eq)]
pub enum GraphCopyRefusal {
    /// The destination graph already holds objects; nothing was written.
    DestinationNotEmpty,
    /// The destination returned bytes whose digest differs from the source; the
    /// destination was left empty.
    DigestMismatch { source: String, destination: String },
    /// The source store could not be read.
    Source(String),
    /// The destination store failed.
    Destination(String),
}

impl GraphCopyRefusal {
    /// The wire code of the refusal.
    pub fn code(&self) -> &'static str {
        match self {
            Self::DestinationNotEmpty => GRAPH_COPY_DESTINATION_NOT_EMPTY,
            Self::DigestMismatch { .. } => GRAPH_COPY_DIGEST_MISMATCH,
            Self::Source(_) | Self::Destination(_) => "GRAPH_COPY_FAILED",
        }
    }
}

impl std::fmt::Display for GraphCopyRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DestinationNotEmpty => write!(
                f,
                "{}: the destination graph already holds objects",
                self.code()
            ),
            Self::DigestMismatch {
                source,
                destination,
            } => write!(
                f,
                "{}: source {source} but destination {destination}",
                self.code()
            ),
            Self::Source(error) | Self::Destination(error) => {
                write!(f, "{}: {error}", self.code())
            }
        }
    }
}

/// What a finished copy wrote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphCopyReport {
    pub batches: u64,
    pub rows: u64,
    /// The whole-graph digest both sides agreed on.
    pub digest: String,
    /// The version `HEAD` names after the copy.
    pub version: u64,
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The whole-graph digest: a chain over each batch's digest and row count, in order.
struct Chain(Sha256);

impl Chain {
    fn new() -> Self {
        Self(Sha256::new())
    }

    fn push(&mut self, batch_digest: &str, rows: u64) {
        self.0.update(batch_digest.as_bytes());
        self.0.update(rows.to_be_bytes());
    }

    fn finish(self) -> String {
        hex(&self.0.finalize())
    }
}

fn destination(error: impl std::fmt::Display) -> GraphCopyRefusal {
    GraphCopyRefusal::Destination(error.to_string())
}

fn base(root: &str, tenant: &str, graph: &str) -> String {
    format!("{root}/{tenant}/{graph}")
}

/// Copy `graph_fname` of the stopped store into `store` under `<root>/<tenant>/<graph>`.
pub async fn copy_graph(
    backend: &RedbBackend,
    graph_fname: &str,
    tenant: &str,
    graph: &str,
    store: Arc<dyn ObjectStore>,
    root: &str,
) -> Result<GraphCopyReport, GraphCopyRefusal> {
    let base = base(root, tenant, graph);
    let prefix = ObjPath::from(base.clone());
    let existing: Vec<_> = store
        .list(Some(&prefix))
        .try_collect()
        .await
        .map_err(destination)?;
    if !existing.is_empty() {
        return Err(GraphCopyRefusal::DestinationNotEmpty);
    }

    // Pass one: stream the source pages, stage each as one segment.
    let mut source_chain = Chain::new();
    let mut staged: Vec<(String, String, u64)> = Vec::new();
    let (mut node_after, mut edge_after) = (None, None);
    loop {
        let Some(page) = backend
            .export_graph_raw_page(graph_fname, node_after.clone(), edge_after.clone(), BATCH_ROWS)
            .map_err(GraphCopyRefusal::Source)?
        else {
            break;
        };
        node_after = page.node_after.clone();
        edge_after = page.edge_after.clone();
        let done = page.nodes_exhausted && page.edges_exhausted;
        let batch = RowBatch {
            nodes: page.nodes,
            edges: page.edges,
        };
        if batch.rows() > 0 {
            let bytes = rmp_serde::to_vec_named(&batch)
                .map_err(|error| GraphCopyRefusal::Source(error.to_string()))?;
            let digest = hex(&Sha256::digest(&bytes));
            let key = format!("{base}/staging/{:08}.seg", staged.len());
            store
                .put(&ObjPath::from(key.clone()), bytes.into())
                .await
                .map_err(destination)?;
            source_chain.push(&digest, batch.rows());
            staged.push((key, digest, batch.rows()));
        }
        if done {
            break;
        }
    }
    let source_digest = source_chain.finish();

    // Pass two: read every staged segment back and recompute the digest.
    let mut destination_chain = Chain::new();
    let mut segments = Vec::with_capacity(staged.len());
    for (key, digest, rows) in &staged {
        let bytes = store
            .get(&ObjPath::from(key.clone()))
            .await
            .map_err(destination)?
            .bytes()
            .await
            .map_err(destination)?;
        let read_back = hex(&Sha256::digest(&bytes));
        let counted = rmp_serde::from_slice::<RowBatch>(&bytes)
            .map(|batch| batch.rows())
            .unwrap_or(0);
        destination_chain.push(&read_back, counted);
        segments.push((digest.clone(), *rows, bytes));
    }
    let destination_digest = destination_chain.finish();
    if destination_digest != source_digest {
        for (key, _, _) in &staged {
            let _ = store.delete(&ObjPath::from(key.clone())).await;
        }
        return Err(GraphCopyRefusal::DigestMismatch {
            source: source_digest,
            destination: destination_digest,
        });
    }

    // Publish: segments under their digests, then the manifest, then `HEAD`.
    let create = || PutOptions {
        mode: PutMode::Create,
        ..PutOptions::default()
    };
    let mut headers = Vec::with_capacity(segments.len());
    for (digest, _, bytes) in &segments {
        let key = format!("{base}/segments/{digest}.seg");
        match store
            .put_opts(&ObjPath::from(key.clone()), bytes.clone().into(), create())
            .await
        {
            Ok(_) | Err(object_store::Error::AlreadyExists { .. }) => {}
            Err(error) => return Err(destination(error)),
        }
        headers.push(SegmentHeader {
            key: digest.clone(),
            sha256: digest.clone(),
        });
    }
    store
        .put(
            &ObjPath::from(format!("{base}/segments/{EMPTY_SHA256}.seg")),
            Vec::new().into(),
        )
        .await
        .map_err(destination)?;
    let manifest = |version: u64, parent: Option<u64>, batches: Vec<SegmentHeader>| {
        let mut manifest = VersionManifest {
            schema_version: 2,
            tenant: tenant.to_string(),
            graph: graph.to_string(),
            version,
            parent_version: parent,
            snapshot: SegmentHeader {
                key: EMPTY_SHA256.to_string(),
                sha256: EMPTY_SHA256.to_string(),
            },
            batches,
            digest: String::new(),
        };
        manifest.digest = manifest.body_digest().map_err(GraphCopyRefusal::Destination)?;
        serde_json::to_vec(&manifest).map_err(destination)
    };
    let version = u64::from(!headers.is_empty());
    store
        .put_opts(
            &ObjPath::from(format!("{base}/manifests/{:020}.json", 0)),
            manifest(0, None, Vec::new())?.into(),
            create(),
        )
        .await
        .map_err(destination)?;
    if version == 1 {
        store
            .put_opts(
                &ObjPath::from(format!("{base}/manifests/{:020}.json", 1)),
                manifest(1, Some(0), headers)?.into(),
                create(),
            )
            .await
            .map_err(destination)?;
    }
    let head = ObjectHead {
        version,
        manifest: format!("manifests/{version:020}.json"),
    };
    store
        .put_opts(
            &ObjPath::from(format!("{base}/HEAD")),
            serde_json::to_vec(&head).map_err(destination)?.into(),
            create(),
        )
        .await
        .map_err(destination)?;
    for (key, _, _) in &staged {
        let _ = store.delete(&ObjPath::from(key.clone())).await;
    }
    Ok(GraphCopyReport {
        batches: staged.len() as u64,
        rows: staged.iter().map(|(_, _, rows)| rows).sum(),
        digest: source_digest,
        version,
    })
}

/// The object root every copied graph sits under: `<ROOT>/<tenant>/<graph>`.
pub const COPY_ROOT: &str = "root";

/// What `graph-copy` was asked to do.
#[derive(Debug, Clone)]
pub struct GraphCopyOptions {
    /// The graph to copy out of the data directory.
    pub graph: String,
    /// The tenant the graph's object keys carry.
    pub tenant: String,
    /// A local directory, or `s3://bucket[/prefix]`, that holds the object root.
    pub to: String,
    /// The explicit confirmation. Without it nothing is opened.
    pub confirm: bool,
}

fn object_store_at(to: &str) -> Result<Arc<dyn ObjectStore>, String> {
    if let Some(rest) = to.strip_prefix("s3://") {
        let (bucket, prefix) = rest.split_once('/').unwrap_or((rest, ""));
        let s3 = object_store::aws::AmazonS3Builder::from_env()
            .with_bucket_name(bucket)
            .build()
            .map_err(|error| format!("GRAPH_COPY_DESTINATION_UNAVAILABLE: {error}"))?;
        return Ok(Arc::new(object_store::prefix::PrefixStore::new(
            s3,
            prefix.trim_matches('/'),
        )));
    }
    std::fs::create_dir_all(to)
        .map_err(|error| format!("GRAPH_COPY_DESTINATION_UNAVAILABLE: {to}: {error}"))?;
    let local = object_store::local::LocalFileSystem::new_with_prefix(to)
        .map_err(|error| format!("GRAPH_COPY_DESTINATION_UNAVAILABLE: {error}"))?;
    Ok(Arc::new(local))
}

/// `store-upgrade graph-copy`: copy one graph of a stopped engine's data directory to the
/// object root. The engine must be stopped: opening the store takes its single-writer lock.
pub fn graph_copy(data_dir: &std::path::Path, options: &GraphCopyOptions) -> super::Report {
    use super::{Report, StoreReport, StoreStatus};
    if !options.confirm {
        return Report::refused(
            "graph-copy",
            data_dir,
            "GRAPH_COPY_NOT_CONFIRMED: graph-copy writes to the object root; pass --confirm to run it"
                .to_string(),
        );
    }
    let result = (|| {
        let store = object_store_at(&options.to)?;
        let backend = RedbBackend::open(data_dir.to_string_lossy().into_owned(), 64)
            .map_err(|error| format!("GRAPH_COPY_SOURCE_UNAVAILABLE: {error}"))?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| error.to_string())?;
        let graph_fname = medulla_kernel::persist::sanitize(&options.graph);
        let copied = runtime.block_on(copy_graph(
            &backend,
            &graph_fname,
            &options.tenant,
            &options.graph,
            store,
            COPY_ROOT,
        ));
        medulla_server_api::api::persistence::PersistenceBackend::shutdown(&backend);
        copied.map_err(|refusal| refusal.to_string())
    })();
    match result {
        Ok(copied) => Report::finished(
            "graph-copy",
            data_dir,
            vec![StoreReport::new(
                &options.graph,
                None,
                StoreStatus::Upgraded,
                format!(
                    "copied {} rows in {} batches to {}, digest {}",
                    copied.rows, copied.batches, options.to, copied.digest
                ),
            )],
            0,
        ),
        Err(error) => Report::refused("graph-copy", data_dir, error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use futures_util::stream::BoxStream;
    use medulla_protocol_types::protocol::Method;
    use medulla_server_api::api::persistence::PersistenceBackend;
    use medulla_storage::object::fetch::ObjectFetcher;
    use object_store::memory::InMemory;

    const GRAPH: &str = "g";

    async fn seeded_backend(dir: &std::path::Path, nodes: usize) -> RedbBackend {
        let backend = RedbBackend::open(dir.to_string_lossy().into_owned(), 64).unwrap();
        for i in 0..nodes {
            backend
                .record_durable(
                    GRAPH,
                    &Method::AddNode {
                        node_id: format!("n{i:05}"),
                        properties_msgpack: rmp_serde::to_vec_named(
                            &serde_json::json!({"type": "Doc", "k": i}),
                        )
                        .unwrap(),
                    },
                )
                .await
                .unwrap();
        }
        backend
    }

    /// An in-memory store that flips one byte of the second staged segment.
    #[derive(Debug)]
    struct Corrupting(InMemory);

    impl std::fmt::Display for Corrupting {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("corrupting")
        }
    }

    #[async_trait]
    impl ObjectStore for Corrupting {
        async fn put_opts(
            &self,
            location: &ObjPath,
            payload: object_store::PutPayload,
            options: PutOptions,
        ) -> object_store::Result<object_store::PutResult> {
            if location.as_ref().ends_with("staging/00000001.seg") {
                let mut bytes: Vec<u8> = payload.iter().flat_map(|b| b.to_vec()).collect();
                let last = bytes.len() - 1;
                bytes[last] ^= 0xff;
                return self.0.put_opts(location, bytes.into(), options).await;
            }
            self.0.put_opts(location, payload, options).await
        }
        async fn put_multipart_opts(
            &self,
            location: &ObjPath,
            options: object_store::PutMultipartOptions,
        ) -> object_store::Result<Box<dyn object_store::MultipartUpload>> {
            self.0.put_multipart_opts(location, options).await
        }
        async fn get_opts(
            &self,
            location: &ObjPath,
            options: object_store::GetOptions,
        ) -> object_store::Result<object_store::GetResult> {
            self.0.get_opts(location, options).await
        }
        fn delete_stream(
            &self,
            locations: BoxStream<'static, object_store::Result<ObjPath>>,
        ) -> BoxStream<'static, object_store::Result<ObjPath>> {
            self.0.delete_stream(locations)
        }
        fn list(
            &self,
            prefix: Option<&ObjPath>,
        ) -> BoxStream<'static, object_store::Result<object_store::ObjectMeta>> {
            self.0.list(prefix)
        }
        async fn list_with_delimiter(
            &self,
            prefix: Option<&ObjPath>,
        ) -> object_store::Result<object_store::ListResult> {
            self.0.list_with_delimiter(prefix).await
        }
        async fn copy_opts(
            &self,
            from: &ObjPath,
            to: &ObjPath,
            options: object_store::CopyOptions,
        ) -> object_store::Result<()> {
            self.0.copy_opts(from, to, options).await
        }
    }

    // spec: MEDULLA-OBJECT-STORAGE-BACKEND-R003.2
    #[tokio::test]
    async fn clean_copy_is_readable_from_the_object_root() {
        let dir = tempfile::tempdir().unwrap();
        let backend = seeded_backend(dir.path(), 2_500).await;
        let store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
        let report = copy_graph(&backend, GRAPH, "t", "g", store.clone(), "root")
            .await
            .unwrap();
        assert_eq!(report.rows, 2_500);
        assert_eq!(report.batches, 3);
        assert_eq!(report.version, 1);
        let fetcher = ObjectFetcher::new(store.clone(), "root");
        let head = fetcher.head("t", "g").await.unwrap();
        let state = fetcher.materialize("t", "g", head.version).await.unwrap();
        let copied: usize = state
            .batches
            .iter()
            .map(|segment| rmp_serde::from_slice::<RowBatch>(&segment.bytes).unwrap().nodes.len())
            .sum();
        assert_eq!(copied, 2_500);
        // A second copy into the same graph is refused and writes nothing.
        let before: Vec<_> = store.list(None).try_collect::<Vec<_>>().await.unwrap();
        let refused = copy_graph(&backend, GRAPH, "t", "g", store.clone(), "root")
            .await
            .unwrap_err();
        assert_eq!(refused.code(), GRAPH_COPY_DESTINATION_NOT_EMPTY);
        let after: Vec<_> = store.list(None).try_collect::<Vec<_>>().await.unwrap();
        assert_eq!(before.len(), after.len());
    }

    // spec: MEDULLA-OBJECT-STORAGE-BACKEND-R003.2
    #[tokio::test]
    async fn digest_mismatch_leaves_destination_empty() {
        let dir = tempfile::tempdir().unwrap();
        let backend = seeded_backend(dir.path(), 2_500).await;
        let store: Arc<dyn ObjectStore> = Arc::new(Corrupting(InMemory::new()));
        let refused = copy_graph(&backend, GRAPH, "t", "g", store.clone(), "root")
            .await
            .unwrap_err();
        assert_eq!(refused.code(), GRAPH_COPY_DIGEST_MISMATCH);
        let left: Vec<_> = store.list(None).try_collect::<Vec<_>>().await.unwrap();
        assert!(left.is_empty(), "no object remains: {left:?}");
    }
}
