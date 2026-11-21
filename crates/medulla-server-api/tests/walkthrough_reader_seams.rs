//! Walkthrough validation reads the snapshot and blobs only through the
//! injected seams (MEDULLA-ORCH-VOCAB-R013.7.4).

use std::sync::Arc;

use medulla_result_types::outcome_bundle::{
    CodeWalkthrough, WalkthroughContent, WalkthroughSection, WalkthroughSourceRef,
};
use medulla_server_api::walkthrough_readers::{
    BlobReader, RepositoryIndexReader, WalkthroughReadRefusal, WalkthroughReaders,
};
use medulla_types::contract::{Digest256, ServerErrorCode};

const ARTIFACT: &[u8] = b"walkthrough artifact";

struct Index;

impl RepositoryIndexReader for Index {
    fn current_revision(&self, tenant: &str, repository_id: &str) -> Option<String> {
        (tenant == "tenant-a" && repository_id == "repo-1").then(|| "rev-2".to_string())
    }

    fn snapshot_contains(
        &self,
        tenant: &str,
        repository_id: &str,
        repository_revision: &str,
        entity_id: &str,
        source_revision: &str,
        source_digest: &Digest256,
    ) -> bool {
        tenant == "tenant-a"
            && repository_id == "repo-1"
            && repository_revision == "rev-2"
            && entity_id == "entity-1"
            && source_revision == "rev-2"
            && *source_digest == Digest256::sha256(b"source")
    }
}

struct Blobs;

impl BlobReader for Blobs {
    fn read(&self, owner_scope: &str, artifact_ref: &str) -> Option<Vec<u8>> {
        (owner_scope == "owner-a" && artifact_ref == "blob:1").then(|| ARTIFACT.to_vec())
    }
}

fn walkthrough() -> CodeWalkthrough {
    CodeWalkthrough {
        run_id: "run-1".to_string(),
        repository_id: "repo-1".to_string(),
        repository_revision: "rev-2".to_string(),
        graph_snapshot_revision: 7,
        query_digest: Digest256::sha256(b"query"),
        ordered_sections: vec![WalkthroughSection {
            ordinal: 0,
            source_refs: vec![WalkthroughSourceRef {
                entity_id: "entity-1".to_string(),
                source_revision: "rev-2".to_string(),
                source_digest: Digest256::sha256(b"source"),
                path: "src/lib.rs".to_string(),
                start_line: 1,
                end_line: 3,
            }],
            content: WalkthroughContent::Referenced {
                digest: Digest256::sha256(b"content"),
            },
        }],
        interaction_edges: Vec::new(),
        artifact_ref: "blob:1".to_string(),
        artifact_digest: Digest256::sha256(ARTIFACT),
        execution_notes: Vec::new(),
    }
}

// spec: MEDULLA-ORCH-VOCAB-R013.7.4
#[test]
fn validation_reads_snapshot_and_blobs_only_through_the_seams() {
    let readers = WalkthroughReaders::new(Arc::new(Index), Arc::new(Blobs));
    assert_eq!(readers.resolve("tenant-a", "owner-a", &walkthrough()), Ok(()));

    let mut stale = walkthrough();
    stale.repository_revision = "rev-1".to_string();
    assert_eq!(
        readers.resolve("tenant-a", "owner-a", &stale),
        Err(WalkthroughReadRefusal::RevisionStale)
    );
    assert_eq!(
        readers.resolve("tenant-b", "owner-b", &walkthrough()),
        Err(WalkthroughReadRefusal::RevisionStale)
    );

    let mut outside = walkthrough();
    outside.ordered_sections[0].source_refs[0].entity_id = "entity-9".to_string();
    assert_eq!(
        readers.resolve("tenant-a", "owner-a", &outside),
        Err(WalkthroughReadRefusal::RefOutsideSnapshot)
    );

    let mut tampered = walkthrough();
    tampered.artifact_digest = Digest256::sha256(b"other");
    assert_eq!(
        readers.resolve("tenant-a", "owner-a", &tampered),
        Err(WalkthroughReadRefusal::ArtifactUnresolved)
    );
    let mut missing = walkthrough();
    missing.artifact_ref = "blob:2".to_string();
    assert_eq!(
        readers.resolve("tenant-a", "owner-a", &missing),
        Err(WalkthroughReadRefusal::ArtifactUnresolved)
    );

    assert_eq!(
        WalkthroughReadRefusal::RevisionStale.code().as_str(),
        "WALKTHROUGH_REVISION_STALE"
    );
    assert_eq!(
        ServerErrorCode::WalkthroughSpanOutsideRun.as_str(),
        "WALKTHROUGH_SPAN_OUTSIDE_RUN"
    );
}
