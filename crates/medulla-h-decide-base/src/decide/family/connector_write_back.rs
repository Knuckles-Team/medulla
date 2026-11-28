//! The `connector_write_back` question family (MEDULLA-DECISION-ENGINE-R001.101).
//!
//! Ranks through the shared matrix ladder; the registry's default hooks
//! carry it, so this family overrides none.

use super::DecisionFamily;

/// The family of the `connector_write_back` question kind.
pub struct ConnectorWriteBackFamily;

impl DecisionFamily for ConnectorWriteBackFamily {}
