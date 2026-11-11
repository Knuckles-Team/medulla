//! The `stage_allocation` question family (MEDULLA-DECISION-ENGINE-R001.101).
//!
//! Ranks through the shared matrix ladder; the registry's default hooks
//! carry it, so this family overrides none.

use super::DecisionFamily;

/// The family of the `stage_allocation` question kind.
pub struct StageAllocationFamily;

impl DecisionFamily for StageAllocationFamily {}
