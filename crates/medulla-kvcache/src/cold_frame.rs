//! The self-describing frame a block takes in the COLD device (MEDULLA-DURABLE-KERNEL-R083.5).
//!
//! A demoted block is more than its stored bytes: the codec and original length rebuild
//! the block, the data version gates freshness and the content digest verifies a page-in.
//! All four travel in the row itself, so a cold row written by one process is readable by
//! the next and the cache can adopt it ([`crate::TieredCache::adopt_cold`]) without any
//! in-memory side table surviving.
//!
//! Layout, big-endian: `[codec u8][orig_len u64][version tag u8][version u64][digest_len u16][digest][data]`.
//! Version tag `0` is [`DataVersion::Agnostic`] (the version field is zero), `1` is
//! [`DataVersion::At`].

use crate::compress::Codec;
use crate::version::DataVersion;

/// Bytes before the digest: codec, original length, version tag, version, digest length.
const FIXED_HEADER: usize = 1 + 8 + 1 + 8 + 2;

/// A decoded cold row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ColdFrame {
    /// Codec `data` was produced with.
    pub codec: Codec,
    /// Original (uncompressed) block length.
    pub orig_len: usize,
    /// Data version the block was derived at.
    pub version: DataVersion,
    /// Content digest of the decoded block recorded at first write (empty when none).
    pub digest: String,
    /// The stored (possibly compressed) bytes.
    pub data: Vec<u8>,
}

/// Frame one cold row.
pub fn encode_cold_frame(frame: &ColdFrame) -> Vec<u8> {
    let digest = frame.digest.as_bytes();
    let digest_len = u16::try_from(digest.len()).unwrap_or(u16::MAX);
    let digest = &digest[..usize::from(digest_len)];
    let (tag, version) = match frame.version {
        DataVersion::Agnostic => (0u8, 0u64),
        DataVersion::At(v) => (1u8, v),
    };
    let mut out = Vec::with_capacity(FIXED_HEADER + digest.len() + frame.data.len());
    out.push(frame.codec.tag());
    out.extend_from_slice(&(frame.orig_len as u64).to_be_bytes());
    out.push(tag);
    out.extend_from_slice(&version.to_be_bytes());
    out.extend_from_slice(&digest_len.to_be_bytes());
    out.extend_from_slice(digest);
    out.extend_from_slice(&frame.data);
    out
}

/// Read one cold row back; `None` when the row is not a well-formed frame.
pub fn decode_cold_frame(raw: &[u8]) -> Option<ColdFrame> {
    if raw.len() < FIXED_HEADER {
        return None;
    }
    let codec = Codec::from_tag(raw[0])?;
    let orig_len = usize::try_from(u64::from_be_bytes(raw[1..9].try_into().ok()?)).ok()?;
    let version_value = u64::from_be_bytes(raw[10..18].try_into().ok()?);
    let version = match raw[9] {
        0 => DataVersion::Agnostic,
        1 => DataVersion::At(version_value),
        _ => return None,
    };
    let digest_len = usize::from(u16::from_be_bytes(raw[18..20].try_into().ok()?));
    let data_at = FIXED_HEADER.checked_add(digest_len)?;
    let digest = String::from_utf8(raw.get(FIXED_HEADER..data_at)?.to_vec()).ok()?;
    Some(ColdFrame {
        codec,
        orig_len,
        version,
        digest,
        data: raw[data_at..].to_vec(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(version: DataVersion) -> ColdFrame {
        ColdFrame {
            codec: Codec::Rle,
            orig_len: 4096,
            version,
            digest: "d1g3st".to_string(),
            data: vec![7, 8, 9],
        }
    }

    /// MEDULLA-DURABLE-KERNEL-R083.5: a cold row keeps its codec, length, version and digest.
    #[test]
    fn cold_frame_round_trips_agnostic_and_versioned() {
        for version in [DataVersion::Agnostic, DataVersion::At(42)] {
            let f = frame(version);
            assert_eq!(decode_cold_frame(&encode_cold_frame(&f)), Some(f));
        }
    }

    /// A truncated or foreign row is not a frame.
    #[test]
    fn cold_frame_rejects_malformed_rows() {
        let good = encode_cold_frame(&frame(DataVersion::Agnostic));
        assert_eq!(decode_cold_frame(&good[..FIXED_HEADER - 1]), None);
        let mut bad_codec = good.clone();
        bad_codec[0] = 0xEE;
        assert_eq!(decode_cold_frame(&bad_codec), None);
        let mut bad_tag = good.clone();
        bad_tag[9] = 9;
        assert_eq!(decode_cold_frame(&bad_tag), None);
        let mut short_digest = good;
        short_digest.truncate(FIXED_HEADER + 2);
        assert_eq!(decode_cold_frame(&short_digest), None);
    }
}
