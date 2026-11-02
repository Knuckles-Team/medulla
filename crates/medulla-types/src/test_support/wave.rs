//! Seeded digests and bounded collections every contract-wave fixture uses.

use crate::contract::{BoundedVec, Digest256};

/// A distinct, well-formed `sha256:<hex>` digest for `seed`.
pub fn digest_text(seed: u8) -> String {
    format!("sha256:{}", hex::encode(raw_digest(seed).as_bytes()))
}

/// A distinct 32-byte digest for `seed`.
pub fn raw_digest(seed: u8) -> Digest256 {
    Digest256::from_bytes([seed; 32])
}

/// Build a bounded collection from a sample that is known to fit.
pub fn bounded<T, const MAXIMUM: usize>(values: Vec<T>) -> BoundedVec<T, MAXIMUM> {
    BoundedVec::new(values).expect("contract-wave samples are inside their declared bounds")
}
