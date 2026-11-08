//! Object-store commit path, commit before ack
//! (MEDULLA-OBJECT-STORAGE-BACKEND-R003).
//!
//! Steps 2–4 of the served commit chain read as one fence: upload the
//! `BatchSegment` of the commit as an immutable, content-addressed segment;
//! put `manifests/<v+1>.json` create-only; then update `HEAD` with a
//! conditional put on its current version tag. Step 4 is the fence — the ack
//! is the store's confirmation of that conditional put, so a crash between
//! the segment upload and the `HEAD` update reopens at the prior version,
//! and a losing conditional put is `OBJECT_PUBLISH_STALE` (the node stops
//! publishing, discards its local commit and re-fetches `HEAD`).
//!
//! Reuses the `VersionManifest` / `ObjectHead` / `SegmentHeader` shapes of
//! the read path (`super::fetch`); the journal is cumulative: each manifest
//! names its snapshot segment and every batch segment after it in commit
//! order. Writes: one immutable segment, one immutable manifest, one
//! conditional `HEAD`.

use object_store::path::Path as ObjPath;
use object_store::{ObjectStore, ObjectStoreExt, PutMode, PutOptions};

use super::fetch::{ObjectHead, SegmentHeader, VersionManifest};

fn hex_digest(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let out = hasher.finalize();
    out.iter().map(|b| format!("{b:02x}")).collect()
}

/// Manifest schema version written by the commit path.
const SCHEMA_VERSION: u32 = 2;

/// Sanitize a logical name into a single object-store path segment.
fn path_segment(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect()
}

/// Typed refusal of one publish. `wire_code` is the literal the wire
/// boundary keeps stable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommitRefusal {
    /// No `HEAD` document to fence on; publishing needs the bootstrapped
    /// manifest of the open path first.
    HeadMissing,
    /// The conditional put lost its fence: another publisher advanced
    /// `HEAD` (or took this manifest version) since it was read.
    PublishStale { version: u64 },
    /// The stored previous manifest no longer hashes to its stated digest.
    DigestMismatch { version: u64, key: String },
    /// The previous manifest is unreadable or not self-consistent.
    InvalidManifest { version: u64, detail: String },
    /// The remote object store failed for a reason outside this row.
    Remote { detail: String },
}

impl CommitRefusal {
    pub fn wire_code(&self) -> &'static str {
        match self {
            Self::PublishStale { .. } => "OBJECT_PUBLISH_STALE",
            Self::DigestMismatch { .. } => "OBJECT_SEGMENT_DIGEST_MISMATCH",
            Self::HeadMissing | Self::InvalidManifest { .. } | Self::Remote { .. } => {
                "OBJECT_HEAD_MISSING"
            }
        }
    }
}

/// Commit-path publisher over any `object_store` implementation. Steps 2–4
/// of MEDULLA-OBJECT-STORAGE-BACKEND-R003; step 1 (the local store commit)
/// and step 5 (the ack) stay with the calling backend.
#[derive(Debug, Clone)]
pub struct ObjectPublisher {
    store: std::sync::Arc<dyn ObjectStore>,
    root: String,
    /// Manifest schema version written by this publisher.
    schema_version: u32,
}

impl ObjectPublisher {
    pub fn new(store: std::sync::Arc<dyn ObjectStore>, root: impl Into<String>) -> Self {
        Self {
            store,
            root: root.into(),
            schema_version: SCHEMA_VERSION,
        }
    }

    fn head_path(&self, tenant: &str, graph: &str) -> ObjPath {
        ObjPath::from(format!(
            "{}/{}/{}/HEAD",
            self.root.trim_matches('/'),
            path_segment(tenant),
            path_segment(graph)
        ))
    }

    fn manifest_path(&self, tenant: &str, graph: &str, version: u64) -> ObjPath {
        ObjPath::from(format!(
            "{}/{}/{}/manifests/{version:020}.json",
            self.root.trim_matches('/'),
            path_segment(tenant),
            path_segment(graph)
        ))
    }

    fn segment_path(&self, tenant: &str, graph: &str, key: &str) -> ObjPath {
        ObjPath::from(format!(
            "{}/{}/{}/segments/{}.seg",
            self.root.trim_matches('/'),
            path_segment(tenant),
            path_segment(graph),
            key
        ))
    }

    async fn get(&self, path: &ObjPath) -> Result<Vec<u8>, CommitRefusal> {
        match self.store.get(path).await {
            Ok(result) => Ok(result
                .bytes()
                .await
                .map_err(|e| CommitRefusal::Remote {
                    detail: e.to_string(),
                })?
                .to_vec()),
            Err(object_store::Error::NotFound { .. }) => Err(CommitRefusal::HeadMissing),
            Err(e) => Err(CommitRefusal::Remote {
                detail: e.to_string(),
            }),
        }
    }

    async fn put_opts(
        &self,
        path: &ObjPath,
        bytes: Vec<u8>,
        options: PutOptions,
    ) -> Result<(), object_store::Error> {
        self.store
            .put_opts(path, bytes.into(), options)
            .await
            .map(|_| ())
    }

    /// Get the current `HEAD` plus the version tag its conditional put must
    /// fence on. An unsigned `None` means no `HEAD` document exists yet.
    async fn fence_head(
        &self,
        tenant: &str,
        graph: &str,
    ) -> Result<Option<(ObjectHead, object_store::UpdateVersion)>, CommitRefusal> {
        let path = self.head_path(tenant, graph);
        match self.store.get_opts(&path, Default::default()).await {
            Ok(result) => {
                let meta = result.meta.clone();
                let bytes = result.bytes().await.map_err(|e| CommitRefusal::Remote {
                    detail: e.to_string(),
                })?;
                let head: ObjectHead =
                    serde_json::from_slice(&bytes).map_err(|e| CommitRefusal::Remote {
                        detail: format!("HEAD decode: {e}"),
                    })?;
                Ok(Some((
                    head,
                    object_store::UpdateVersion {
                        e_tag: meta.e_tag,
                        version: meta.version,
                    },
                )))
            }
            Err(object_store::Error::NotFound { .. }) => Ok(None),
            Err(e) => Err(CommitRefusal::Remote {
                detail: e.to_string(),
            }),
        }
    }

    async fn read_previous_manifest(
        &self,
        tenant: &str,
        graph: &str,
        version: u64,
    ) -> Result<VersionManifest, CommitRefusal> {
        let key = format!("manifests/{version:020}.json");
        let bytes = self
            .get(&self.manifest_path(tenant, graph, version))
            .await
            .map_err(|r| match r {
                CommitRefusal::HeadMissing => CommitRefusal::InvalidManifest {
                    version,
                    detail: format!("previous manifest {key} absent"),
                },
                other => other,
            })?;
        let manifest: VersionManifest =
            serde_json::from_slice(&bytes).map_err(|e| CommitRefusal::InvalidManifest {
                version,
                detail: format!("manifest decode: {e}"),
            })?;
        if manifest.version != version {
            return Err(CommitRefusal::InvalidManifest {
                version,
                detail: format!("manifest self-reports version {}", manifest.version),
            });
        }
        let stated = manifest
            .body_digest()
            .map_err(|e| CommitRefusal::Remote { detail: e })?;
        if manifest.digest != stated {
            return Err(CommitRefusal::DigestMismatch { version, key });
        }
        Ok(manifest)
    }

    /// Steps 2–4: upload this commit's `BatchSegment` as an immutable,
    /// content-addressed segment, put the next manifest create-only, then
    /// conditionally update `HEAD`. Returns the `HEAD` as the fence confirmed
    /// it, carrying the new version the caller may ack after step 4. The
    /// `batch_segment` bytes are the encoded segment payload verified in
    /// place; the caller holds the already-durable local copy.
    pub async fn publish(
        &self,
        tenant: &str,
        graph: &str,
        batch_segment: &[u8],
    ) -> Result<ObjectHead, CommitRefusal> {
        let (prior_head, fence) = match self.fence_head(tenant, graph).await? {
            Some(pair) => pair,
            None => return Err(CommitRefusal::HeadMissing),
        };
        let prior_version = prior_head.version;
        let prior = self
            .read_previous_manifest(tenant, graph, prior_version)
            .await?;

        // Step 2: the batch segment is immutable and content-addressed; a
        // byte-identical existing object is the same object.
        let segment_key = hex_digest(batch_segment);
        self.store
            .put(
                &self.segment_path(tenant, graph, &segment_key),
                batch_segment.to_vec().into(),
            )
            .await
            .map_err(|e| CommitRefusal::Remote {
                detail: e.to_string(),
            })?;

        // Step 3: the next manifest, create-only. Losing this put before the
        // `HEAD` fence still leaves `HEAD` unchanged — the unacked commit
        // stays absent from the root.
        let mut manifest = VersionManifest {
            schema_version: self.schema_version,
            tenant: tenant.to_string(),
            graph: graph.to_string(),
            version: prior_version + 1,
            parent_version: Some(prior_version),
            snapshot: prior.snapshot,
            batches: {
                let mut batches = prior.batches;
                batches.push(SegmentHeader {
                    key: segment_key.clone(),
                    sha256: segment_key.clone(),
                });
                batches
            },
            digest: String::new(),
        };
        manifest.digest = manifest
            .body_digest()
            .map_err(|e| CommitRefusal::Remote { detail: e })?;
        let next_version = manifest.version;
        let manifest_key = format!("manifests/{next_version:020}.json");
        if let Err(e) = self
            .put_opts(
                &self.manifest_path(tenant, graph, next_version),
                serde_json::to_vec(&manifest).map_err(|e| CommitRefusal::Remote {
                    detail: e.to_string(),
                })?,
                PutOptions {
                    mode: PutMode::Create,
                    ..PutOptions::default()
                },
            )
            .await
        {
            return Err(match e {
                object_store::Error::AlreadyExists { .. }
                | object_store::Error::Precondition { .. } => CommitRefusal::PublishStale {
                    version: prior_version,
                },
                other => CommitRefusal::Remote {
                    detail: other.to_string(),
                },
            });
        }

        // Step 4: the fence. A conditional put on the `HEAD` version tag
        // read above; a losing put means another publisher advanced the
        // graph and this node's local commit must be discarded.
        let head = ObjectHead {
            version: next_version,
            manifest: manifest_key.clone(),
        };
        let head_bytes = serde_json::to_vec(&head).map_err(|e| CommitRefusal::Remote {
            detail: e.to_string(),
        })?;
        if let Err(e) = self
            .put_opts(
                &self.head_path(tenant, graph),
                head_bytes,
                PutOptions {
                    mode: PutMode::Update(fence),
                    ..PutOptions::default()
                },
            )
            .await
        {
            return Err(match e {
                object_store::Error::Precondition { .. }
                | object_store::Error::AlreadyExists { .. } => CommitRefusal::PublishStale {
                    version: prior_version,
                },
                other => CommitRefusal::Remote {
                    detail: other.to_string(),
                },
            });
        }

        Ok(ObjectHead {
            version: next_version,
            manifest: manifest_key,
        })
    }
}
