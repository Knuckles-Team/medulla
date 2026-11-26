//! The `item_recommendation` family's registered abstentions, the key of the
//! encoder a catalog is fitted into and the receipt of that fit
//! (MEDULLA-DECISION-ENGINE-R175.3, R175.4, R175.8).

use serde::{Deserialize, Serialize};

use crate::decision::record::outcome::RefusalCode;

/// Abstention: the catalog graph holds no embedded item.
pub const EMPTY_CATALOG: RefusalCode = RefusalCode::new("EMPTY_CATALOG");
/// Abstention: the query vector's width differs from the catalog's.
pub const DIMENSION_MISMATCH: RefusalCode = RefusalCode::new("DIMENSION_MISMATCH");
/// Abstention: no encoder was ever fitted for the catalog reference.
pub const ENCODER_MISSING: RefusalCode = RefusalCode::new("ENCODER_MISSING");
/// Abstention: the encoders fitted for the catalog reference are older than its
/// current digest.
pub const ENCODER_STALE: RefusalCode = RefusalCode::new("ENCODER_STALE");

/// The `item_recommendation` family table; it joins `codes::FAMILY_TABLES`.
pub const REFUSALS: &[RefusalCode] = &[
    EMPTY_CATALOG,
    DIMENSION_MISMATCH,
    ENCODER_MISSING,
    ENCODER_STALE,
];

/// The key prefix every encoder of one catalog reference shares.
pub fn encoder_prefix(catalog_ref: &str) -> String {
    format!("recommend-encoder:{catalog_ref}:")
}

/// The key of the encoder fitted for `catalog_ref` at `catalog_digest` (64
/// lowercase hex characters): an encoder is content-addressed by the catalog it
/// was fitted on, so a changed catalog never overwrites it.
pub fn encoder_key(catalog_ref: &str, catalog_digest: &str) -> String {
    format!("{}{catalog_digest}", encoder_prefix(catalog_ref))
}

/// What one `FitCatalogEncoder` write stored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct CatalogEncoderFitted {
    pub catalog_ref: String,
    /// SHA-256 over the sorted item ids and vectors, as 64 lowercase hex characters.
    pub catalog_digest: String,
    /// How many embedded items the catalog held.
    pub items: u32,
    /// The width of every item vector.
    pub dimension: u32,
    /// How many codebook levels the encoder holds.
    pub levels: u8,
}
