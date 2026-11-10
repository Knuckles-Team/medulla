//! Archive segment put and get by sequence range
//! (MEDULLA-OBJECT-STORAGE-BACKEND-R006).
//!
//! An archive segment holds `BatchSegment` bytes as an immutable,
//! content-checked object at `<root>/archive/<tenant>/<graph>/<first:020>-<last:020>.seg`.
//! The manifest entry carries a SHA-256 digest; `put_segment` refuses an
//! overlapping range with `ARCHIVE_RANGE_OVERLAP` and `get_segment` refuses a
//! digest mismatch with `ARCHIVE_DIGEST_MISMATCH` (an existing wire code).
//! Provider for `MEDULLA-ENGINE-PARITY-R016.1`. Writes: one immutable object
//! per put, plus the manifest document.

use object_store::path::Path as ObjPath;
use object_store::{ObjectStore, ObjectStoreExt};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// One archived segment in the manifest. `first` inclusive, `last` exclusive.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArchiveManifestEntry {
    pub key: String,
    pub first: u64,
    pub last: u64,
    /// 64 lowercase hex characters.
    pub sha256: String,
}

/// Typed refusal. `wire_code` is the literal the wire boundary keeps stable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArchiveRefusal {
    /// The put's range overlaps an existing archived segment.
    RangeOverlap { first: u64, last: u64 },
    /// The stored bytes no longer hash to the manifest digest.
    DigestMismatch { first: u64, last: u64 },
    /// No manifest entry (or no object) for the requested range.
    SegmentMissing { first: u64, last: u64 },
    /// The remote object store failed for a reason outside this row.
    Remote { detail: String },
}

impl ArchiveRefusal {
    pub fn wire_code(&self) -> &'static str {
        match self {
            Self::RangeOverlap { .. } => "ARCHIVE_RANGE_OVERLAP",
            Self::DigestMismatch { .. } => "ARCHIVE_DIGEST_MISMATCH",
            Self::SegmentMissing { .. } => "ARCHIVE_MISSING",
            Self::Remote { .. } => "ARCHIVE_MISSING",
        }
    }
}

fn hex_digest(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let out = hasher.finalize();
    out.iter().map(|b| format!("{b:02x}")).collect()
}

/// Sanitize a logical name into a single object-store path segment.
fn path_segment(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect()
}

/// Immutable archive segments over any `object_store` implementation.
#[derive(Debug, Clone)]
pub struct ObjectArchive {
    store: std::sync::Arc<dyn ObjectStore>,
    root: String,
}

impl ObjectArchive {
    pub fn new(store: std::sync::Arc<dyn ObjectStore>, root: impl Into<String>) -> Self {
        Self {
            store,
            root: root.into(),
        }
    }

    fn base(&self, tenant: &str, graph: &str) -> String {
        format!(
            "{}/archive/{}/{}/",
            self.root.trim_matches('/'),
            path_segment(tenant),
            path_segment(graph)
        )
    }

    fn manifest_path(&self, tenant: &str, graph: &str) -> ObjPath {
        ObjPath::from(format!("{}manifest.mp", self.base(tenant, graph)))
    }

    fn segment_path(&self, tenant: &str, graph: &str, first: u64, last: u64) -> ObjPath {
        ObjPath::from(format!(
            "{}{:020}-{:020}.seg",
            self.base(tenant, graph),
            first,
            last
        ))
    }

    async fn read_manifest(
        &self,
        tenant: &str,
        graph: &str,
    ) -> Result<Vec<ArchiveManifestEntry>, ArchiveRefusal> {
        let path = self.manifest_path(tenant, graph);
        match self.store.get(&path).await {
            Ok(r) => {
                let bytes = r.bytes().await.map_err(|e| ArchiveRefusal::Remote {
                    detail: e.to_string(),
                })?;
                rmp_serde::from_slice(&bytes).map_err(|e| ArchiveRefusal::Remote {
                    detail: format!("archive manifest decode: {e}"),
                })
            }
            Err(object_store::Error::NotFound { .. }) => Ok(Vec::new()),
            Err(e) => Err(ArchiveRefusal::Remote {
                detail: e.to_string(),
            }),
        }
    }

    async fn write_manifest(
        &self,
        tenant: &str,
        graph: &str,
        entries: &[ArchiveManifestEntry],
    ) -> Result<(), ArchiveRefusal> {
        let bytes = rmp_serde::to_vec_named(entries).map_err(|e| ArchiveRefusal::Remote {
            detail: format!("archive manifest encode: {e}"),
        })?;
        self.store
            .put(&self.manifest_path(tenant, graph), bytes.into())
            .await
            .map_err(|e| ArchiveRefusal::Remote {
                detail: e.to_string(),
            })?;
        Ok(())
    }

    async fn put_object(&self, path: &ObjPath, bytes: Vec<u8>) -> Result<(), ArchiveRefusal> {
        self.store
            .put(path, bytes.into())
            .await
            .map(|_| ())
            .map_err(|e| ArchiveRefusal::Remote {
                detail: e.to_string(),
            })
    }

    /// Put one immutable segment covering `[first, last)` with a verified
    /// digest: refuses an overlapping range before any write, then writes the
    /// object, reads it back, verifies the digest, and finally appends the
    /// manifest entry.
    pub async fn put_segment(
        &self,
        tenant: &str,
        graph: &str,
        first: u64,
        last: u64,
        bytes: &[u8],
    ) -> Result<String, ArchiveRefusal> {
        let entries = self.read_manifest(tenant, graph).await?;
        if entries.iter().any(|e| e.first < last && first < e.last) {
            return Err(ArchiveRefusal::RangeOverlap { first, last });
        }
        let key = format!("{first:020}-{last:020}.seg");
        let path = self.segment_path(tenant, graph, first, last);
        let digest = hex_digest(bytes);
        self.put_object(&path, bytes.to_vec()).await?;
        let stored = match self.store.get(&path).await {
            Ok(r) => r.bytes().await.map_err(|e| ArchiveRefusal::Remote {
                detail: e.to_string(),
            }),
            Err(e) => Err(ArchiveRefusal::Remote {
                detail: e.to_string(),
            }),
        };
        match stored {
            Ok(b) => {
                if hex_digest(&b) != digest {
                    let _ = self.store.delete(&path).await;
                    return Err(ArchiveRefusal::DigestMismatch { first, last });
                }
            }
            Err(e) => return Err(e),
        }
        let mut entries = entries;
        entries.push(ArchiveManifestEntry {
            key: key.clone(),
            first,
            last,
            sha256: digest,
        });
        self.write_manifest(tenant, graph, &entries).await?;
        Ok(key)
    }

    /// Get the segment covering exactly `[first, last)` and verify it against
    /// the manifest digest before returning the bytes.
    pub async fn get_segment(
        &self,
        tenant: &str,
        graph: &str,
        first: u64,
        last: u64,
    ) -> Result<Vec<u8>, ArchiveRefusal> {
        let entries = self.read_manifest(tenant, graph).await?;
        let entry = entries
            .iter()
            .find(|e| e.first == first && e.last == last)
            .cloned();
        let entry = entry.ok_or(ArchiveRefusal::SegmentMissing { first, last })?;
        let path = self.segment_path(tenant, graph, first, last);
        let plain = match self.store.get(&path).await {
            Ok(r) => r
                .bytes()
                .await
                .map_err(|e| ArchiveRefusal::Remote {
                    detail: e.to_string(),
                })?
                .to_vec(),
            Err(object_store::Error::NotFound { .. }) => {
                return Err(ArchiveRefusal::SegmentMissing { first, last })
            }
            Err(e) => {
                return Err(ArchiveRefusal::Remote {
                    detail: e.to_string(),
                })
            }
        };
        if hex_digest(&plain) != entry.sha256 {
            return Err(ArchiveRefusal::DigestMismatch { first, last });
        }
        Ok(plain)
    }
}
