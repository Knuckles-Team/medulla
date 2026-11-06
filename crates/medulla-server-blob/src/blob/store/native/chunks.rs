//! Chunk-byte reads and bounded group-commit writes for the native CAS.

use super::super::*;

pub(super) fn put_chunk(store: &RedbChunkStore, bytes: &[u8]) -> Result<(String, bool), String> {
    let digest = manifest::chunk_digest(bytes)?;
    let mut batch = store.batch.lock();
    // Dedup: already staged in this window, or already committed to `cas_chunks`.
    if batch.pending.contains_key(&digest) || store.chunk_present(&digest)? {
        return Ok((digest, false));
    }
    batch.pending.insert(digest.clone(), bytes.to_vec());
    // Group-commit boundary: flush every `group` staged chunks so resident
    // chunk bodies never exceed the configured group window.
    if batch.pending.len() >= batch.group {
        store.commit_group(&mut batch)?;
    }
    Ok((digest, true))
}

pub(super) fn get_chunk(store: &RedbChunkStore, digest: &str) -> Result<Option<Vec<u8>>, String> {
    // A read must see all committed chunks: flush the open group first so a
    // just-uploaded chunk is durable + visible (a staged group is not yet a row).
    store.flush_chunks()?;
    store.shared_read()?.chunk_bytes(digest)
}

/// Replace chunk `digest`'s committed bytes (delete, then insert under the same
/// address) in one admitted maintenance write. Test support only.
#[cfg(any(test, feature = "test-support"))]
pub(super) fn corrupt_chunk_for_test(
    store: &RedbChunkStore,
    digest: &str,
    bytes: &[u8],
) -> Result<(), String> {
    store.flush_chunks()?;
    let at = medulla_kernel::kernel::dispatch_support::authoritative_now_ms();
    store.maintain("blob_chunk_corrupt_for_test", digest, at, |wtx| {
        let shared = shared_write(wtx, &store.shared)?;
        if !shared.remove_chunk(digest)? {
            return Err(format!("chunk {digest} is not committed"));
        }
        shared.insert_chunks_if_absent(&[(digest, bytes)])?;
        Ok(())
    })
}
