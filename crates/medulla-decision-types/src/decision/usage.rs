//! Refusal codes of the per-tenant usage ledger (MEDULLA-DECISION-ENGINE-R801.4).
//!
//! The ledger is not a question family, so its table is not keyed by a
//! question kind; `codes::resolve` reads it beside the shared codes. A code is
//! returned before any ledger row is written.

use super::record::outcome::RefusalCode;

/// A record `tenant` differs from the verified tenant.
pub const USAGE_UNKNOWN_TENANT: RefusalCode = RefusalCode::new("USAGE_UNKNOWN_TENANT");
/// A wire count is negative.
pub const USAGE_NEGATIVE_COUNT: RefusalCode = RefusalCode::new("USAGE_NEGATIVE_COUNT");
/// `tokens_cached` exceeds `tokens_in`.
pub const USAGE_CACHED_EXCEEDS_INPUT: RefusalCode = RefusalCode::new("USAGE_CACHED_EXCEEDS_INPUT");
/// The batch exceeds `usage.max_batch`.
pub const USAGE_BATCH_TOO_LARGE: RefusalCode = RefusalCode::new("USAGE_BATCH_TOO_LARGE");
/// A `content_digest` is not 64 lowercase hex characters.
pub const USAGE_BAD_DIGEST: RefusalCode = RefusalCode::new("USAGE_BAD_DIGEST");
/// A new price entry starts at or before the latest entry of its key with other prices.
pub const PRICE_WINDOW_OVERLAP: RefusalCode = RefusalCode::new("PRICE_WINDOW_OVERLAP");
/// The ledger store failed; nothing was written.
pub const USAGE_STORE_FAILED: RefusalCode = RefusalCode::new("USAGE_STORE_FAILED");

/// The usage ledger's own refusal codes.
pub const REFUSALS: &[RefusalCode] = &[
    PRICE_WINDOW_OVERLAP,
    USAGE_BAD_DIGEST,
    USAGE_BATCH_TOO_LARGE,
    USAGE_CACHED_EXCEEDS_INPUT,
    USAGE_NEGATIVE_COUNT,
    USAGE_STORE_FAILED,
    USAGE_UNKNOWN_TENANT,
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decision::codes;

    #[test]
    fn usage_codes_are_registered_and_distinct() {
        for code in REFUSALS {
            assert_eq!(codes::resolve(code.as_str()), Some(*code));
            assert!(!codes::SHARED.contains(code));
        }
        let unique: std::collections::BTreeSet<_> = REFUSALS.iter().collect();
        assert_eq!(unique.len(), REFUSALS.len());
    }
}
