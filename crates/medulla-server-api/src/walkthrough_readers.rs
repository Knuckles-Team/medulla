//! Repository-index and blob reader seams for walkthrough manifest validation
//! (MEDULLA-ORCH-VOCAB-R013.7.4).
//!
//! The `AddNode` handler resolves a walkthrough's repository revision, source
//! refs and artifact through these injected readers before graph admission
//! runs, so validation never reaches into storage itself. Every read is scoped
//! by tenant: another tenant's snapshot or blob reads as absent.

use std::sync::Arc;

use medulla_result_types::outcome_bundle::CodeWalkthrough;
use medulla_types::contract::{Digest256, ServerErrorCode};

/// Reads the pinned repository snapshot.
pub trait RepositoryIndexReader: Send + Sync {
    /// The current indexed revision of `repository_id` for `tenant`, or
    /// `None` when the repository is absent for that tenant.
    fn current_revision(&self, tenant: &str, repository_id: &str) -> Option<String>;

    /// Whether `entity_id` at `source_revision` with `source_digest` is part
    /// of the pinned snapshot of `repository_id` for `tenant`.
    fn snapshot_contains(
        &self,
        tenant: &str,
        repository_id: &str,
        repository_revision: &str,
        entity_id: &str,
        source_revision: &str,
        source_digest: &Digest256,
    ) -> bool;
}

/// Reads an artifact by its reference.
pub trait BlobReader: Send + Sync {
    /// The artifact bytes, or `None` when the reference names nothing for
    /// `owner_scope`, the carrier owner scope that stored the upload.
    fn read(&self, owner_scope: &str, artifact_ref: &str) -> Option<Vec<u8>>;
}

/// The three checks that need the repository index or the blob store, run
/// before admission. The handler passes the result on to the graph-only
/// validation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WalkthroughReadRefusal {
    /// The repository revision is missing or is not the current one.
    RevisionStale,
    /// A source ref is outside the pinned snapshot.
    RefOutsideSnapshot,
    /// The artifact is unresolved or its bytes do not match its digest.
    ArtifactUnresolved,
}

impl WalkthroughReadRefusal {
    /// The registered wire code.
    pub fn code(self) -> ServerErrorCode {
        match self {
            Self::RevisionStale => ServerErrorCode::WalkthroughRevisionStale,
            Self::RefOutsideSnapshot => ServerErrorCode::WalkthroughRefOutsideSnapshot,
            Self::ArtifactUnresolved => ServerErrorCode::WalkthroughArtifactUnresolved,
        }
    }
}

/// The two injected readers.
#[derive(Clone)]
pub struct WalkthroughReaders {
    index: Arc<dyn RepositoryIndexReader>,
    blobs: Arc<dyn BlobReader>,
}

/// Install the walkthrough readers once; a second install returns the rejected value.
pub fn install_walkthrough_readers(
    seams: &crate::api::registry::SeamRegistry,
    readers: WalkthroughReaders,
) -> Result<(), WalkthroughReaders> {
    seams.walkthrough.set(readers)
}

impl WalkthroughReaders {
    /// Inject the readers.
    pub fn new(index: Arc<dyn RepositoryIndexReader>, blobs: Arc<dyn BlobReader>) -> Self {
        Self { index, blobs }
    }

    /// Resolve `walkthrough` for `tenant` through the seams only. The artifact is read
    /// under `blob_owner`, the carrier owner scope of the writing request, because that
    /// is the owner every served upload is stored under.
    pub fn resolve(
        &self,
        tenant: &str,
        blob_owner: &str,
        walkthrough: &CodeWalkthrough,
    ) -> Result<(), WalkthroughReadRefusal> {
        match self
            .index
            .current_revision(tenant, &walkthrough.repository_id)
        {
            Some(current) if current == walkthrough.repository_revision => {}
            _ => return Err(WalkthroughReadRefusal::RevisionStale),
        }
        for section in &walkthrough.ordered_sections {
            for source in &section.source_refs {
                if !self.index.snapshot_contains(
                    tenant,
                    &walkthrough.repository_id,
                    &walkthrough.repository_revision,
                    &source.entity_id,
                    &source.source_revision,
                    &source.source_digest,
                ) {
                    return Err(WalkthroughReadRefusal::RefOutsideSnapshot);
                }
            }
        }
        match self.blobs.read(blob_owner, &walkthrough.artifact_ref) {
            Some(bytes) if Digest256::sha256(&bytes) == walkthrough.artifact_digest => Ok(()),
            _ => Err(WalkthroughReadRefusal::ArtifactUnresolved),
        }
    }
}
