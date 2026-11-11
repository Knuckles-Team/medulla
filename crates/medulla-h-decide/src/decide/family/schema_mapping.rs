//! The `schema_mapping` question family (MEDULLA-DECISION-ENGINE-R001.101).
//!
//! Ranks through the shared matrix ladder; the registry's default hooks
//! carry it, so this family overrides none.

use super::DecisionFamily;

/// The family of the `schema_mapping` question kind.
pub struct SchemaMappingFamily;

impl DecisionFamily for SchemaMappingFamily {}
