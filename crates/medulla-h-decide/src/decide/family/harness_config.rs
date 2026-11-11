//! The `harness_config` question family (MEDULLA-DECISION-ENGINE-R001.101).
//!
//! Ranks through the shared matrix ladder; the registry's default hooks
//! carry it, so this family overrides none.

use super::DecisionFamily;

/// The family of the `harness_config` question kind.
pub struct HarnessConfigFamily;

impl DecisionFamily for HarnessConfigFamily {}
