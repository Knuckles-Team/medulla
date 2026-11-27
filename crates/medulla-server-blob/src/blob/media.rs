//! Tenant-scoped media objects over the one content-addressed store
//! (MEDULLA-MEDIA-VOCAB-R657).
//!
//! A media object is one chunk of the CAS keyed by the SHA-256 of its bytes. A tenant "has" the
//! object while it holds the named holder `media:<hex(tenant)>:<sha256>`; no second store, table
//! or index exists. Identical bytes give the same digest and the same holder, so a repeated put
//! is a no-op. A read checks the caller's own holder, so a digest that was never stored, one
//! that only another tenant holds and one whose holder was tombstoned all refuse alike with
//! [`MEDIA_NOT_FOUND`]: there is no existence oracle.

use super::store::{hex_digest, ChunkStore, HolderId};

/// The one refusal of every unreadable digest: absent, foreign, tombstoned or malformed.
pub const MEDIA_NOT_FOUND: &str = "MEDIA_NOT_FOUND";
/// A put over the `media.max_bytes` knob.
pub const MEDIA_TOO_LARGE: &str = "MEDIA_TOO_LARGE";
/// A put with no bytes or a media type that is not `type/subtype`.
pub const MEDIA_INVALID: &str = "MEDIA_INVALID";

/// The holder that records `tenant_id`'s hold of the object with SHA-256 `digest`.
pub fn media_holder(tenant_id: &str, digest: &str) -> Result<HolderId, String> {
    HolderId::new(&format!(
        "media:{}:{}",
        hex_digest(tenant_id.as_bytes()),
        digest
    ))
}

/// Whether `digest` is 64 lowercase hex characters.
pub fn is_sha256_hex(digest: &str) -> bool {
    digest.len() == 64
        && digest
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Check a put's bytes and media type against `max_bytes` (the `media.max_bytes` knob).
pub fn validate_put(media_type: &str, bytes: &[u8], max_bytes: u64) -> Result<(), String> {
    let token = |part: &str| {
        !part.is_empty()
            && part
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"!#$&^_.+-".contains(&b))
    };
    let typed = media_type
        .split_once('/')
        .is_some_and(|(kind, sub)| token(kind) && token(sub));
    if bytes.is_empty() || !typed {
        return Err(format!(
            "{MEDIA_INVALID}: a media put needs bytes and a type/subtype media type"
        ));
    }
    if bytes.len() as u64 > max_bytes {
        return Err(format!(
            "{MEDIA_TOO_LARGE}: {} bytes exceed the media.max_bytes limit of {max_bytes}",
            bytes.len()
        ));
    }
    Ok(())
}

/// Read the object `digest` as `tenant_id`, verifying its bytes against the digest. Every miss is
/// [`MEDIA_NOT_FOUND`] with no detail.
pub fn read_media(
    store: &dyn ChunkStore,
    tenant_id: &str,
    digest: &str,
) -> Result<Vec<u8>, String> {
    let missing = || MEDIA_NOT_FOUND.to_string();
    if !is_sha256_hex(digest) {
        return Err(missing());
    }
    let holder = media_holder(tenant_id, digest)?;
    if !store.has_named_holder(digest, holder.as_str())? {
        return Err(missing());
    }
    match store.get_chunk(digest)? {
        Some(bytes) if hex_digest(&bytes) == digest => Ok(bytes),
        _ => Err(missing()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blob::store::test_support::fresh;
    use crate::blob::store::{HolderChange, RedbChunkStore};
    use crate::blob::ENGINE_BLOB_OWNER_SCOPE;

    fn put(store: &RedbChunkStore, tenant: &str, bytes: &[u8], op: &str) -> String {
        let (digest, _) = store.put_chunk(bytes).unwrap();
        let change = HolderChange::acquire(
            &digest,
            media_holder(tenant, &digest).unwrap(),
            ENGINE_BLOB_OWNER_SCOPE,
        )
        .unwrap();
        store.holder_batch(&change, &fresh(store, op), 1).unwrap();
        digest
    }

    // spec: MEDULLA-MEDIA-VOCAB-R657.2
    #[test]
    fn a_foreign_missing_and_malformed_digest_refuse_alike() {
        let store = RedbChunkStore::open_temp().unwrap();
        let digest = put(&store, "tenant-a", b"png bytes", "put-a");
        assert_eq!(
            read_media(&store, "tenant-a", &digest).unwrap(),
            b"png bytes"
        );
        let absent = hex_digest(b"never stored");
        let refusals = [
            read_media(&store, "tenant-b", &digest),
            read_media(&store, "tenant-a", &absent),
            read_media(&store, "tenant-a", "not-a-digest"),
        ];
        for refusal in refusals {
            assert_eq!(refusal.unwrap_err(), MEDIA_NOT_FOUND);
        }
    }

    // spec: MEDULLA-MEDIA-VOCAB-R657.1
    #[test]
    fn identical_bytes_share_one_digest_and_one_holder() {
        let store = RedbChunkStore::open_temp().unwrap();
        let first = put(&store, "tenant-a", b"same", "put-1");
        let second = put(&store, "tenant-a", b"same", "put-2");
        assert_eq!(first, second);
        assert_eq!(store.refcount(&first).unwrap(), 1);
        // A second tenant holding the same bytes adds its own holder, never reads the first's.
        put(&store, "tenant-b", b"same", "put-3");
        assert_eq!(store.refcount(&first).unwrap(), 2);
    }

    // spec: MEDULLA-MEDIA-VOCAB-R657.3
    #[test]
    fn a_tombstone_makes_the_digest_read_as_missing_and_is_idempotent() {
        let store = RedbChunkStore::open_temp().unwrap();
        let digest = put(&store, "tenant-a", b"expiring", "put-a");
        let release =
            HolderChange::release(&digest, media_holder("tenant-a", &digest).unwrap()).unwrap();
        assert!(
            store
                .holder_batch(&release, &fresh(&store, "tomb-1"), 2)
                .unwrap()
                .changed
        );
        assert!(
            !store
                .holder_batch(&release, &fresh(&store, "tomb-2"), 3)
                .unwrap()
                .changed
        );
        assert_eq!(
            read_media(&store, "tenant-a", &digest).unwrap_err(),
            MEDIA_NOT_FOUND
        );
    }

    // spec: MEDULLA-MEDIA-VOCAB-R657.1
    #[test]
    fn put_validation_applies_the_knob_limit_and_type_shape() {
        assert!(validate_put("image/png", b"x", 1).is_ok());
        assert!(validate_put("image/png", b"xy", 1)
            .unwrap_err()
            .starts_with(MEDIA_TOO_LARGE));
        assert!(validate_put("image/png", b"", 8)
            .unwrap_err()
            .starts_with(MEDIA_INVALID));
        assert!(validate_put("png", b"x", 8)
            .unwrap_err()
            .starts_with(MEDIA_INVALID));
    }
}
