//! Source-changeset blob intake (MEDULLA-SCM-R001): the content-addressed
//! blob store phase of `SubmitChangeset`.
//!
//! File bytes reach the CAS *before* the changeset through the existing
//! `BlobBegin`/`BlobChunkPut`/`BlobCommit` protocol ([`crate::blob`]), and
//! `BlobCommit` returns the digest; the changeset request carries no bytes,
//! only `SubmittedBlobRef`s. This module verifies every ref against the SAME
//! content-addressed chunk store the upload protocol writes through, so a ref
//! the CAS cannot re-derive byte for byte is refused before any changeset
//! record is written:
//!
//! - `size` over `changeset_max_blob_bytes` refuses `BLOB_TOO_LARGE` before
//!   any store read;
//! - an unknown or unreadable digest refuses `BLOB_NOT_FOUND`;
//! - a stored length different from `size`, or a chunk digest the bytes read
//!   back through `stream_blob_get` re-derive differently, refuses
//!   `BLOB_DIGEST_MISMATCH`.
//!
//! Identical bytes store once in the CAS: re-verifying an already-stored ref,
//! or a second upload of the same content, returns the same digest and adds
//! no storage.
//!
//! Path-set acceptance (MEDULLA-SCM-R003.1): the accept phase decides, by
//! the shared `medulla_transaction::occ::disjoint` over the two write-set
//! footprints built from the path sets, whether the submitted changeset
//! shares no path with the changesets after `base_head`. Disjoint path sets
//! land in parallel with no branch-wide lock; an overlap enters the
//! three-way merge (MEDULLA-BRANCH-MERGE-R004). No second disjointness
//! check exists on the accept path.

use std::io::Write;

use medulla_server_blob::blob::store::{BlobManifest, ChunkStore};
use medulla_server_blob::blob::stream::stream_blob_get;
use medulla_transaction::occ::{KeyRange, TxnFootprint};
use medulla_types::source_changeset::{
    ChangedPath, Changeset, ChangesetRefusal, PathOp, SubmitChangesetRequest, SubmitRefusal,
    SubmittedBlobRef, Trailers,
};
use sha2::{Digest, Sha256};

/// A `Write` that re-derives chunk digests from the bytes `stream_blob_get`
/// hands over, against the manifest's recorded chunk digests: one running
/// SHA-256 per chunk, compared at every chunk boundary. The first divergence
/// is remembered; no bytes are retained.
struct ChunkVerifier<'m> {
    manifest: &'m BlobManifest,
    chunk: usize,
    consumed: u64,
    hash: Sha256,
    mismatch: bool,
}

impl<'m> ChunkVerifier<'m> {
    fn new(manifest: &'m BlobManifest) -> Self {
        Self {
            manifest,
            chunk: 0,
            consumed: 0,
            hash: Sha256::new(),
            mismatch: false,
        }
    }

    /// True when the streamed bytes diverged from the manifest's chunk
    /// digests or lengths, or the stream ended mid-chunk.
    fn mismatch(&self) -> bool {
        self.mismatch || self.chunk != self.manifest.chunks.len()
    }

    /// Compare the finished chunk's derived digest with the recorded one.
    fn close_chunk(&mut self) {
        let expected = &self.manifest.chunks[self.chunk];
        let derived = hex::encode(self.hash.finalize_reset());
        if derived != *expected {
            self.mismatch = true;
            return;
        }
        self.chunk += 1;
        self.consumed = 0;
    }
}

impl Write for ChunkVerifier<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if !self.mismatch {
            let mut off = 0usize;
            while off < buf.len() && !self.mismatch {
                if self.chunk >= self.manifest.chunks.len() {
                    // More bytes than the manifest claims.
                    self.mismatch = true;
                    break;
                }
                let boundary = self.manifest.chunk_lens[self.chunk] as u64;
                if self.consumed == boundary {
                    // Only reachable for a zero-length chunk: verify it.
                    self.close_chunk();
                    continue;
                }
                let take = (boundary - self.consumed).min(buf.len() as u64 - off as u64) as usize;
                self.hash.update(&buf[off..off + take]);
                self.consumed += take as u64;
                off += take;
                if self.consumed == boundary {
                    self.close_chunk();
                }
            }
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        if !self.mismatch && self.consumed != 0 {
            self.mismatch = true; // stream ended mid-chunk
        }
        Ok(())
    }
}

/// Verify the submitted blob refs of one changeset against the CAS
/// (MEDULLA-SCM-R001).
///
/// `SubmitChangeset` carries refs, not bytes: each blob was uploaded first
/// through `BlobBegin`/`BlobChunkPut`/`BlobCommit`, whose `BlobCommit`
/// returned the digest the ref names. Every ref is checked, in order:
///
/// 1. `size` over `changeset_max_blob_bytes` — `BLOB_TOO_LARGE`, before any
///    store read;
/// 2. an unknown or unreadable digest — `BLOB_NOT_FOUND`;
/// 3. a stored length different from `size`, or chunk digests that the bytes
///    read back through [`stream_blob_get`] re-derive differently —
///    `BLOB_DIGEST_MISMATCH`.
///
/// The CAS stores identical bytes once, so a ref whose bytes are already
/// stored verifies without adding storage.
pub fn accept_blobs(
    store: &dyn ChunkStore,
    refs: &[SubmittedBlobRef],
    changeset_max_blob_bytes: u64,
) -> Result<(), ChangesetRefusal> {
    for submitted in refs {
        // The limit is enforced before any store read.
        if submitted.size > changeset_max_blob_bytes {
            return Err(ChangesetRefusal::BlobTooLarge);
        }
        let manifest = match store.get_manifest(&submitted.digest) {
            Ok(Some(manifest)) => manifest,
            Ok(None) | Err(_) => return Err(ChangesetRefusal::BlobNotFound),
        };
        if manifest.len != submitted.size {
            return Err(ChangesetRefusal::BlobDigestMismatch);
        }
        // Re-derive the chunk digests from the stored bytes, one chunk at a
        // time, without retaining them.
        let mut verifier = ChunkVerifier::new(&manifest);
        stream_blob_get(store, &submitted.digest, &mut verifier)
            .map_err(|_| ChangesetRefusal::BlobNotFound)?;
        if verifier.mismatch() {
            return Err(ChangesetRefusal::BlobDigestMismatch);
        }
    }
    Ok(())
}

/// The table of the changeset-chain key space: one point key per changed
/// path.
const PATH_SET_TABLE: &str = "source_changeset_paths";

/// Build the write-set [`TxnFootprint`] of a changeset path set
/// (MEDULLA-SCM-R003.1).
///
/// Each changed path is a point key in the chain's path table, so two
/// footprints share a key range exactly when the two changesets touch the
/// same path. Reads stay empty: a changeset's concurrency claim is the set
/// of paths it writes.
fn path_set_footprint(paths: &[String]) -> TxnFootprint {
    TxnFootprint {
        snapshot_seq: 0,
        reads: Vec::new(),
        writes: paths
            .iter()
            .map(|path| KeyRange::point(PATH_SET_TABLE, path.as_bytes()))
            .collect(),
    }
}

/// Path-set acceptance (MEDULLA-SCM-R003.1): `true` when the submitted
/// changeset's path set shares no path with any changeset after `base_head`.
///
/// The decision is the shared `medulla_transaction::occ::disjoint` over the
/// two write-set footprints built from the path sets — the only
/// disjointness check on the accept path. Disjoint path sets land in
/// parallel with no branch-wide lock; an overlap enters the three-way merge
/// of MEDULLA-BRANCH-MERGE-R004 per overlapping path, so the engine never
/// rebases silently.
pub fn path_set_disjoint(submitted: &[String], since_base: &[Vec<String>]) -> bool {
    let since = TxnFootprint {
        snapshot_seq: 0,
        reads: Vec::new(),
        writes: since_base
            .iter()
            .flat_map(|paths| paths.iter())
            .map(|path| KeyRange::point(PATH_SET_TABLE, path.as_bytes()))
            .collect(),
    };
    medulla_transaction::occ::disjoint(&path_set_footprint(submitted), &since)
}

/// Stage one of `SubmitChangeset` (MEDULLA-SCM-R002): the request bounds and
/// pairings, then every blob ref against the CAS. Reads no chain and writes
/// nothing, so a refusal leaves the chain unchanged and the slow blob
/// verification stays outside the registry write transaction.
pub fn verify_submission(
    store: &dyn ChunkStore,
    request: &SubmitChangesetRequest,
    changeset_max_blob_bytes: u64,
) -> Result<(), SubmitRefusal> {
    request.validate()?;
    let refs: Vec<SubmittedBlobRef> = request
        .paths
        .iter()
        .filter_map(|entry| entry.blob.clone())
        .collect();
    accept_blobs(store, &refs, changeset_max_blob_bytes)
        .map_err(|refusal| SubmitRefusal::new(refusal, "a submitted blob ref failed verification"))
}

/// The branch chain as `accept` reads it: the head commit id and the
/// changeset a commit carries (`None` for the branch base pointer).
pub trait ChainView {
    fn head(&self) -> &str;
    fn changeset_at(&self, commit_id: &str) -> Result<Option<Changeset>, String>;
}

/// Why `accept` produced no changeset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AcceptError {
    Refused(SubmitRefusal),
    Store(String),
}

impl From<SubmitRefusal> for AcceptError {
    fn from(refusal: SubmitRefusal) -> Self {
        Self::Refused(refusal)
    }
}

/// The server-side facts of one accept: never read from the request.
pub struct AcceptContext<'a> {
    pub author_principal: &'a str,
    pub created_at_ms: u64,
}

/// Stage two of `SubmitChangeset` (MEDULLA-SCM-R002): extend the changeset
/// chain. Walks the chain from the head, checks the writer's base, fills
/// `old_blob` of every path from the first parent, and seals the record.
///
/// A base behind the head lands only when the submission shares no path with
/// the changesets after it (`path_set_disjoint`, MEDULLA-SCM-R003.1); an
/// overlap refuses, because the engine never rebases silently.
pub fn accept(
    chain: &dyn ChainView,
    request: &SubmitChangesetRequest,
    ctx: &AcceptContext<'_>,
) -> Result<Changeset, AcceptError> {
    let head = chain.head().to_string();
    // Newest first: (commit id, changeset) down to the base pointer.
    let mut walked: Vec<(String, Changeset)> = Vec::new();
    let mut cursor = head.clone();
    while let Some(found) = chain.changeset_at(&cursor).map_err(AcceptError::Store)? {
        let next = found.parents.first().cloned().unwrap_or_default();
        walked.push((cursor, found));
        cursor = next;
    }
    let not_on_branch = |detail: &str| {
        AcceptError::Refused(SubmitRefusal::new(
            ChangesetRefusal::ChangesetParentNotOnBranch,
            detail,
        ))
    };
    let behind = match &request.base_head {
        None if walked.is_empty() => 0,
        None => {
            return Err(not_on_branch(
                "base_head is absent but the branch has changesets",
            ))
        }
        Some(base) => walked
            .iter()
            .position(|(id, _)| id == base)
            .ok_or_else(|| not_on_branch("base_head is not a changeset of the branch"))?,
    };
    let mut touched: Vec<String> = Vec::new();
    for entry in &request.paths {
        touched.push(entry.path.clone());
        touched.extend(entry.renamed_from.clone());
    }
    if behind > 0 {
        let since: Vec<Vec<String>> = walked[..behind]
            .iter()
            .map(|(_, c)| c.changed_paths.iter().map(|p| p.path.clone()).collect())
            .collect();
        if !path_set_disjoint(&touched, &since) {
            return Err(not_on_branch(
                "base_head is behind the head and the path sets overlap",
            ));
        }
    }
    // The blob a path holds at the head: the newest entry naming it.
    let blob_at_head = |path: &str| -> Option<String> {
        walked
            .iter()
            .find_map(|(_, c)| c.changed_paths.iter().find(|p| p.path == path))
            .and_then(|p| p.new_blob.clone())
    };
    let mut changed_paths = Vec::with_capacity(request.paths.len());
    for entry in &request.paths {
        let new_blob = entry.blob.as_ref().map(|b| b.digest.clone());
        match (&entry.op, &entry.renamed_from) {
            (PathOp::Rename, Some(from)) => {
                // A rename is a Rename at the new path and a Delete at the old.
                let old = blob_at_head(from);
                changed_paths.push(ChangedPath {
                    path: entry.path.clone(),
                    op: PathOp::Rename,
                    old_blob: old.clone(),
                    new_blob,
                    mode: entry.mode,
                });
                changed_paths.push(ChangedPath {
                    path: from.clone(),
                    op: PathOp::Delete,
                    old_blob: old,
                    new_blob: None,
                    mode: entry.mode,
                });
            }
            _ => changed_paths.push(ChangedPath {
                path: entry.path.clone(),
                op: entry.op,
                old_blob: blob_at_head(&entry.path),
                new_blob,
                mode: entry.mode,
            }),
        }
    }
    let trailers = request
        .trailers
        .clone()
        .unwrap_or_else(|| Trailers::parse(&request.message));
    Ok(Changeset {
        id: String::new(),
        repo: request.repo.clone(),
        parents: vec![head],
        author_principal: ctx.author_principal.to_string(),
        message: request.message.clone(),
        changed_paths,
        created_at: ctx.created_at_ms,
        trailers,
        agent_identity: None,
        run_trace_id: request.run_trace_id.clone(),
    }
    .sealed())
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_server_blob::blob::store::{
        hex_digest, RedbChunkStore, BLOB_MANIFEST_VERSION, ENGINE_BLOB_OWNER_SCOPE,
    };
    use medulla_server_blob::blob::stream::stream_blob_put;
    use medulla_types::scheduler_config::DEFAULT_CHANGESET_MAX_BLOB_BYTES;
    use std::io::Cursor;

    /// Upload `data` through the existing streaming facade and build the ref
    /// its `BlobCommit` digest yields.
    fn upload(store: &RedbChunkStore, data: &[u8]) -> SubmittedBlobRef {
        let blob = stream_blob_put(store, Cursor::new(data.to_vec()), 4096).unwrap();
        assert_eq!(blob.manifest.len, data.len() as u64);
        SubmittedBlobRef {
            digest: blob.digest,
            size: blob.manifest.len,
        }
    }

    // spec: MEDULLA-SCM-R001
    #[test]
    fn identical_bytes_store_once_and_verify_by_digest() {
        let store = RedbChunkStore::open_temp().unwrap();
        let data: Vec<u8> = (0u32..120_000).map(|i| (i % 211) as u8).collect();
        let submitted = upload(&store, &data);

        let chunks_before = store.chunk_count().unwrap();
        let blobs_before = store.blob_count().unwrap();
        accept_blobs(
            &store,
            std::slice::from_ref(&submitted),
            DEFAULT_CHANGESET_MAX_BLOB_BYTES,
        )
        .unwrap();

        // A second write of the same bytes returns the same digest and adds no storage.
        let again = stream_blob_put(&store, Cursor::new(data.clone()), 4096).unwrap();
        assert_eq!(again.digest, submitted.digest);
        assert_eq!(store.chunk_count().unwrap(), chunks_before);
        assert_eq!(store.blob_count().unwrap(), blobs_before);

        // The digest read-back equals the original bytes.
        let mut out = Vec::new();
        assert_eq!(
            stream_blob_get(&store, &submitted.digest, &mut out).unwrap(),
            data.len() as u64
        );
        assert_eq!(out, data);
    }

    // spec: MEDULLA-SCM-R001
    #[test]
    fn size_over_limit_refuses_before_any_read() {
        let store = RedbChunkStore::open_temp().unwrap();
        // Unknown digest AND oversized: the limit check must fire first.
        let oversized = SubmittedBlobRef {
            digest: "0".repeat(64),
            size: DEFAULT_CHANGESET_MAX_BLOB_BYTES + 1,
        };
        assert_eq!(
            accept_blobs(
                &store,
                std::slice::from_ref(&oversized),
                DEFAULT_CHANGESET_MAX_BLOB_BYTES
            ),
            Err(ChangesetRefusal::BlobTooLarge)
        );
        // Exactly at the limit is not over: it fails later, at the store read.
        let at_limit = SubmittedBlobRef {
            digest: "0".repeat(64),
            size: DEFAULT_CHANGESET_MAX_BLOB_BYTES,
        };
        assert_eq!(
            accept_blobs(
                &store,
                std::slice::from_ref(&at_limit),
                DEFAULT_CHANGESET_MAX_BLOB_BYTES
            ),
            Err(ChangesetRefusal::BlobNotFound)
        );
    }

    // spec: MEDULLA-SCM-R001
    #[test]
    fn unknown_digest_refuses_not_found() {
        let store = RedbChunkStore::open_temp().unwrap();
        let unknown = SubmittedBlobRef {
            digest: "0".repeat(64),
            size: 10,
        };
        assert_eq!(
            accept_blobs(
                &store,
                std::slice::from_ref(&unknown),
                DEFAULT_CHANGESET_MAX_BLOB_BYTES
            ),
            Err(ChangesetRefusal::BlobNotFound)
        );
    }

    // spec: MEDULLA-SCM-R001
    #[test]
    fn declared_size_mismatch_refuses_digest_mismatch() {
        let store = RedbChunkStore::open_temp().unwrap();
        let data: Vec<u8> = (0u32..90_000).map(|i| (i % 89) as u8).collect();
        let stored = upload(&store, &data);
        let wrong_size = SubmittedBlobRef {
            digest: stored.digest,
            size: stored.size + 1,
        };
        assert_eq!(
            accept_blobs(
                &store,
                std::slice::from_ref(&wrong_size),
                DEFAULT_CHANGESET_MAX_BLOB_BYTES
            ),
            Err(ChangesetRefusal::BlobDigestMismatch)
        );
    }

    // spec: MEDULLA-SCM-R001
    #[test]
    fn stored_bytes_rederiving_a_different_digest_refuses() {
        let store = RedbChunkStore::open_temp().unwrap();
        // A real chunk, addressable by its content digest.
        let content: Vec<u8> = (0u32..40_000).map(|i| (i % 97) as u8).collect();
        let (chunk_digest, _) = store.put_chunk(&content).unwrap();
        // A manifest that names that chunk but records half its length: the
        // bytes `stream_blob_get` serves re-derive a different chunk digest.
        let manifest = BlobManifest {
            schema_version: BLOB_MANIFEST_VERSION,
            owner_scope: ENGINE_BLOB_OWNER_SCOPE.to_string(),
            chunks: vec![chunk_digest],
            chunk_lens: vec![content.len() as u32 / 2],
            len: content.len() as u64 / 2,
            chunk_size: 0,
        };
        let manifest_bytes = rmp_serde::to_vec_named(&manifest).unwrap();
        let blob_digest = hex_digest(&manifest_bytes);
        store.put_manifest(&blob_digest, &manifest).unwrap();
        let submitted = SubmittedBlobRef {
            digest: blob_digest,
            size: manifest.len,
        };
        assert_eq!(
            accept_blobs(
                &store,
                std::slice::from_ref(&submitted),
                DEFAULT_CHANGESET_MAX_BLOB_BYTES
            ),
            Err(ChangesetRefusal::BlobDigestMismatch)
        );
    }

    // spec: MEDULLA-SCM-R003.1
    #[test]
    fn disjoint_path_sets_are_disjoint_and_overlapping_sets_are_not() {
        // Two changesets sharing no path range are disjoint: both land
        // with no branch-wide lock.
        assert!(path_set_disjoint(
            &["src/a.rs".into(), "docs/b.md".into()],
            &[vec!["src/c.rs".into()]]
        ));
        // An overlap on one path is not: it enters the three-way merge.
        assert!(!path_set_disjoint(
            &["src/a.rs".into()],
            &[vec!["src/a.rs".into()]]
        ));
        // The overlap may sit in any changeset after the base.
        assert!(!path_set_disjoint(
            &["src/a.rs".into()],
            &[vec!["x/y.rs".into()], vec!["src/a.rs".into()]]
        ));
        // A fresh base (no changeset after it) is disjoint of any submission.
        assert!(path_set_disjoint(&["src/a.rs".into()], &[]));
        // A path is a point key: a directory entry is not its files.
        assert!(path_set_disjoint(
            &["src/a.rs".into()],
            &[vec!["src/".into()]]
        ));
    }

    // spec: MEDULLA-SCM-R003.1
    #[test]
    fn path_set_footprint_feeds_the_shared_occ_disjoint() {
        // The decision is the shared `occ::disjoint` over write-set
        // footprints: point keys in the chain's path table, no reads.
        let fp = path_set_footprint(&["src/a.rs".to_string(), "src/b.rs".to_string()]);
        assert_eq!(fp.snapshot_seq, 0);
        assert_eq!(fp.reads, Vec::<KeyRange>::new());
        assert_eq!(
            fp.writes,
            vec![
                KeyRange::point(PATH_SET_TABLE, b"src/a.rs"),
                KeyRange::point(PATH_SET_TABLE, b"src/b.rs"),
            ]
        );
        // The module's decision equals the shared check over the same
        // construction, on both the disjoint and the overlapping case.
        let submitted = vec!["src/a.rs".to_string()];
        let cases: [(Vec<String>, bool); 2] = [
            (vec!["src/b.rs".to_string()], true),
            (vec!["src/b.rs".to_string(), "src/a.rs".to_string()], false),
        ];
        for (since, expected) in cases {
            let since_fp = TxnFootprint {
                snapshot_seq: 0,
                reads: Vec::new(),
                writes: since
                    .iter()
                    .map(|path| KeyRange::point(PATH_SET_TABLE, path.as_bytes()))
                    .collect(),
            };
            assert_eq!(
                path_set_disjoint(&submitted, &[since.clone()]),
                medulla_transaction::occ::disjoint(&path_set_footprint(&submitted), &since_fp),
                "the decision must equal the shared occ::disjoint verdict"
            );
            assert_eq!(path_set_disjoint(&submitted, &[since]), expected);
        }
    }

    struct MemChain {
        head: String,
        commits: Vec<(String, Changeset)>,
    }

    impl ChainView for MemChain {
        fn head(&self) -> &str {
            &self.head
        }
        fn changeset_at(&self, commit_id: &str) -> Result<Option<Changeset>, String> {
            Ok(self
                .commits
                .iter()
                .find(|(id, _)| id == commit_id)
                .map(|(_, c)| c.clone()))
        }
    }

    fn hex_id(c: char) -> String {
        c.to_string().repeat(64)
    }

    fn submitted(
        path: &str,
        op: PathOp,
        blob: Option<&str>,
    ) -> medulla_types::source_changeset::SubmittedPath {
        medulla_types::source_changeset::SubmittedPath {
            path: path.into(),
            op,
            blob: blob.map(|d| SubmittedBlobRef {
                digest: d.into(),
                size: 1,
            }),
            mode: 0o100644,
            renamed_from: None,
        }
    }

    fn request(
        base: Option<String>,
        paths: Vec<medulla_types::source_changeset::SubmittedPath>,
    ) -> SubmitChangesetRequest {
        SubmitChangesetRequest {
            repo: "r".into(),
            branch: "b".into(),
            base_head: base,
            message: "m\n\nSpec: MEDULLA-SCM-R002".into(),
            paths,
            trailers: None,
            run_trace_id: None,
        }
    }

    const CTX: AcceptContext<'static> = AcceptContext {
        author_principal: "p",
        created_at_ms: 9,
    };

    // spec: MEDULLA-SCM-R002
    #[test]
    fn accept_stores_only_changed_paths_and_fills_old_blobs_from_the_parent() {
        let empty = MemChain {
            head: "base".into(),
            commits: vec![],
        };
        let first = accept(
            &empty,
            &request(
                None,
                vec![
                    submitted("a.rs", PathOp::Add, Some("d1")),
                    submitted("b.rs", PathOp::Add, Some("d2")),
                    submitted("c.rs", PathOp::Add, Some("d3")),
                ],
            ),
            &CTX,
        )
        .unwrap();
        assert_eq!(first.parents, vec!["base".to_string()]);
        assert_eq!(first.id, first.canonical_id());
        assert_eq!(first.trailers.spec_ids, ["MEDULLA-SCM-R002"]);
        let chain = MemChain {
            head: hex_id('1'),
            commits: vec![(hex_id('1'), first)],
        };
        let second = accept(
            &chain,
            &request(
                Some(hex_id('1')),
                vec![
                    submitted("a.rs", PathOp::Modify, Some("d4")),
                    submitted("c.rs", PathOp::Delete, None),
                ],
            ),
            &CTX,
        )
        .unwrap();
        // Two path entries of three files, with old blobs from the first parent.
        assert_eq!(second.changed_paths.len(), 2);
        assert_eq!(second.changed_paths[0].old_blob.as_deref(), Some("d1"));
        assert_eq!(second.changed_paths[0].new_blob.as_deref(), Some("d4"));
        assert_eq!(second.changed_paths[1].old_blob.as_deref(), Some("d3"));
        assert_eq!(second.changed_paths[1].new_blob, None);
        assert_eq!(second.parents, vec![hex_id('1')]);
        assert_eq!(second.id, second.canonical_id());
    }

    // spec: MEDULLA-SCM-R002
    #[test]
    fn accept_refuses_a_base_off_the_branch_and_an_overlapping_stale_base() {
        let first = accept(
            &MemChain {
                head: "base".into(),
                commits: vec![],
            },
            &request(None, vec![submitted("a.rs", PathOp::Add, Some("d1"))]),
            &CTX,
        )
        .unwrap();
        let mut second = first.clone();
        second.parents = vec![hex_id('1')];
        second.changed_paths = vec![ChangedPath {
            path: "z.rs".into(),
            op: PathOp::Add,
            old_blob: None,
            new_blob: Some("d9".into()),
            mode: 0o100644,
        }];
        let chain = MemChain {
            head: hex_id('2'),
            commits: vec![(hex_id('1'), first), (hex_id('2'), second)],
        };
        let refused = |base: Option<String>, path: &str| match accept(
            &chain,
            &request(base, vec![submitted(path, PathOp::Add, Some("d5"))]),
            &CTX,
        ) {
            Err(AcceptError::Refused(r)) => r.refusal,
            other => panic!("expected a refusal, got {other:?}"),
        };
        let off = ChangesetRefusal::ChangesetParentNotOnBranch;
        assert_eq!(refused(Some(hex_id('9')), "n.rs"), off);
        assert_eq!(refused(None, "n.rs"), off);
        // Behind the head but disjoint: lands. Overlapping: refused.
        assert!(accept(
            &chain,
            &request(
                Some(hex_id('1')),
                vec![submitted("n.rs", PathOp::Add, Some("d5"))]
            ),
            &CTX
        )
        .is_ok());
        assert_eq!(refused(Some(hex_id('1')), "z.rs"), off);
    }

    // spec: MEDULLA-SCM-R002
    #[test]
    fn verify_submission_refuses_empty_invalid_and_missing_blobs() {
        let store = RedbChunkStore::open_temp().unwrap();
        let limit = DEFAULT_CHANGESET_MAX_BLOB_BYTES;
        let code = |request: &SubmitChangesetRequest| {
            verify_submission(&store, request, limit)
                .unwrap_err()
                .refusal
                .code()
        };
        assert_eq!(code(&request(None, vec![])), "CHANGESET_EMPTY");
        let delete_with_blob = submitted("a.rs", PathOp::Delete, Some("d"));
        assert_eq!(
            code(&request(None, vec![delete_with_blob])),
            "CHANGESET_INVALID"
        );
        let dup = vec![
            submitted("a.rs", PathOp::Add, Some("d")),
            submitted("a.rs", PathOp::Add, Some("d")),
        ];
        assert_eq!(code(&request(None, dup)), "CHANGESET_INVALID");
        let traversal = submitted("../a.rs", PathOp::Add, Some("d"));
        assert_eq!(code(&request(None, vec![traversal])), "CHANGESET_INVALID");
        let unknown = submitted("a.rs", PathOp::Add, Some(&"f".repeat(64)));
        assert_eq!(code(&request(None, vec![unknown])), "BLOB_NOT_FOUND");
        let data = upload(&store, b"hello");
        let known = medulla_types::source_changeset::SubmittedPath {
            blob: Some(data),
            ..submitted("a.rs", PathOp::Add, None)
        };
        assert!(verify_submission(&store, &request(None, vec![known]), limit).is_ok());
    }
}
