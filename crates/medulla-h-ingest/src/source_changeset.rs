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

use std::io::Write;

use medulla_server_blob::blob::store::{BlobManifest, ChunkStore};
use medulla_server_blob::blob::stream::stream_blob_get;
use medulla_types::source_changeset::{ChangesetRefusal, SubmittedBlobRef};
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
}
