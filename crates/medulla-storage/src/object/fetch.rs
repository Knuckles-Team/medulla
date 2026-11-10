//! Object-store read path for material graph versions
//! (MEDULLA-OBJECT-STORAGE-BACKEND-R002).
//!
//! Layout under `<root>/<tenant>/<graph>/`: `HEAD` holds
//! `{version, manifest}`; `manifests/<version:020>.json` is an immutable
//! `VersionManifest`; `segments/<sha256>.seg` holds content-addressed
//! segments. `ObjectFetcher::materialize(version)` reads manifest `version`
//! and fetches its snapshot and batch segments, verifying every SHA-256. It
//! never writes, copies or compacts the root: the local store is a cache the
//! root overrides. Refusals are typed (`OBJECT_SEGMENT_MISSING`,
//! `OBJECT_SEGMENT_DIGEST_MISMATCH`, version and key each) and stay within the
//! one `object_store` trait that the S3 clients already use.

use object_store::path::Path as ObjPath;
use object_store::{ObjectStore, ObjectStoreExt};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

fn hex_digest(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let out = hasher.finalize();
    out.iter().map(|b| format!("{b:02x}")).collect()
}

fn is_lower_hex_sha256(s: &str) -> bool {
    s.len() == 64 && s.chars().all(|c| c.is_ascii_hexdigit()) && s == s.to_lowercase()
}

/// Sanitize a logical name into a single object-store path segment.
fn path_segment(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect()
}

/// Typed refusal of one materialize. `wire_code` is the literal the wire
/// boundary keeps stable; every domain refusal names the version and key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ObjectReadRefusal {
    /// No HEAD document for the graph.
    HeadMissing,
    /// A manifest-version or segment object the manifest names is absent.
    SegmentMissing { version: u64, key: String },
    /// The stored bytes no longer hash to the manifest digest.
    DigestMismatch { version: u64, key: String },
    /// A manifest is not self-consistent (bad digest shape, key not derived).
    InvalidManifest { version: u64, detail: String },
    /// The remote object store failed for a reason outside this row.
    Remote { detail: String },
}

impl ObjectReadRefusal {
    pub fn wire_code(&self) -> &'static str {
        match self {
            Self::HeadMissing
            | Self::SegmentMissing { .. }
            | Self::InvalidManifest { .. }
            | Self::Remote { .. } => "OBJECT_SEGMENT_MISSING",
            Self::DigestMismatch { .. } => "OBJECT_SEGMENT_DIGEST_MISMATCH",
        }
    }
}

/// One segment entry inside a manifest: a content-addressed key and its
/// SHA-256 digest (64 lowercase hex characters).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SegmentHeader {
    pub key: String,
    pub sha256: String,
}

/// An immutable manifest of one graph version.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VersionManifest {
    pub schema_version: u32,
    pub tenant: String,
    pub graph: String,
    pub version: u64,
    pub parent_version: Option<u64>,
    /// The owner-table snapshot segment of the declared owner tables.
    pub snapshot: SegmentHeader,
    /// The replayed batch segments, in commit order.
    pub batches: Vec<SegmentHeader>,
    /// Digest over the manifest body with `digest` empty.
    pub digest: String,
}

impl VersionManifest {
    /// Digest of the manifest body: a msgpack re-encode with `digest` blank.
    pub fn body_digest(&self) -> Result<String, String> {
        let mut body = self.clone();
        body.digest = String::new();
        let bytes =
            rmp_serde::to_vec_named(&body).map_err(|e| format!("manifest body encode: {e}"))?;
        Ok(hex_digest(&bytes))
    }
}

/// The HEAD document of one graph: the current version and the manifest key.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ObjectHead {
    pub version: u64,
    pub manifest: String,
}

/// One verified segment: `{ key, bytes, sha256 }`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegmentRef {
    pub key: String,
    pub bytes: Vec<u8>,
    pub sha256: String,
}

/// A fully materialized graph version: the verified snapshot and batch
/// segments of its manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaterializedVersion {
    pub version: u64,
    pub parent_version: Option<u64>,
    pub snapshot: SegmentRef,
    pub batches: Vec<SegmentRef>,
}

/// Read-only fetcher over any `object_store` implementation. Writes: none.
#[derive(Debug, Clone)]
pub struct ObjectFetcher {
    store: std::sync::Arc<dyn ObjectStore>,
    root: String,
}

impl ObjectFetcher {
    pub fn new(store: std::sync::Arc<dyn ObjectStore>, root: impl Into<String>) -> Self {
        Self {
            store,
            root: root.into(),
        }
    }

    pub async fn head(&self, tenant: &str, graph: &str) -> Result<ObjectHead, ObjectReadRefusal> {
        let head_key = "HEAD";
        let bytes = self
            .get(&self.head_path(tenant, graph), 0, head_key)
            .await?;
        serde_json::from_slice::<ObjectHead>(&bytes).map_err(|e| ObjectReadRefusal::Remote {
            detail: format!("HEAD decode: {e}"),
        })
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

    async fn get(
        &self,
        path: &ObjPath,
        version: u64,
        key: &str,
    ) -> Result<Vec<u8>, ObjectReadRefusal> {
        match self.store.get(path).await {
            Ok(result) => Ok(result
                .bytes()
                .await
                .map_err(|e| ObjectReadRefusal::Remote {
                    detail: e.to_string(),
                })?
                .to_vec()),
            Err(object_store::Error::NotFound { .. }) => Err(ObjectReadRefusal::SegmentMissing {
                version,
                key: key.to_string(),
            }),
            Err(e) => Err(ObjectReadRefusal::Remote {
                detail: e.to_string(),
            }),
        }
    }

    /// Fetch and verify one content-addressed segment.
    async fn verified_segment(
        &self,
        tenant: &str,
        graph: &str,
        version: u64,
        header: &SegmentHeader,
    ) -> Result<SegmentRef, ObjectReadRefusal> {
        if !is_lower_hex_sha256(&header.sha256) || header.key != header.sha256 {
            return Err(ObjectReadRefusal::InvalidManifest {
                version,
                detail: format!("segment key/digest not content-addressed: {}", header.key),
            });
        }
        let bytes = self
            .get(
                &self.segment_path(tenant, graph, &header.key),
                version,
                &header.key,
            )
            .await?;
        if hex_digest(&bytes) != header.sha256 {
            return Err(ObjectReadRefusal::DigestMismatch {
                version,
                key: header.key.clone(),
            });
        }
        Ok(SegmentRef {
            key: header.key.clone(),
            bytes,
            sha256: header.sha256.clone(),
        })
    }

    /// Read manifest `version` and fetch its snapshot and batch segments,
    /// verifying each SHA-256. A read never writes to the root. A corrupted
    /// segment refuses the read of this version only; earlier versions stay
    /// readable.
    pub async fn materialize(
        &self,
        tenant: &str,
        graph: &str,
        version: u64,
    ) -> Result<MaterializedVersion, ObjectReadRefusal> {
        let manifest_key = format!("manifests/{version:020}.json");
        let bytes = self
            .get(
                &self.manifest_path(tenant, graph, version),
                version,
                &manifest_key,
            )
            .await?;
        let manifest: VersionManifest =
            serde_json::from_slice(&bytes).map_err(|e| ObjectReadRefusal::Remote {
                detail: format!("manifest decode: {e}"),
            })?;
        let stated = manifest
            .body_digest()
            .map_err(|e| ObjectReadRefusal::Remote { detail: e })?;
        if manifest.tenant != tenant || manifest.graph != graph {
            return Err(ObjectReadRefusal::InvalidManifest {
                version,
                detail: "manifest tenant/graph disagree with the fetcher root".to_string(),
            });
        }
        if manifest.digest != stated {
            return Err(ObjectReadRefusal::DigestMismatch {
                version,
                key: manifest_key,
            });
        }
        let snapshot = self
            .verified_segment(tenant, graph, version, &manifest.snapshot)
            .await?;
        let mut batches = Vec::with_capacity(manifest.batches.len());
        for header in &manifest.batches {
            batches.push(
                self.verified_segment(tenant, graph, version, header)
                    .await?,
            );
        }
        Ok(MaterializedVersion {
            version: manifest.version,
            parent_version: manifest.parent_version,
            snapshot,
            batches,
        })
    }
}
