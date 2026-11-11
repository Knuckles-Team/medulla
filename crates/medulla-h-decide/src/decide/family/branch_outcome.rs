//! The `branch_outcome` question family (MEDULLA-DECISION-ENGINE-R001.101).
//!
//! Ranks through the shared matrix ladder; the registry's default hooks
//! carry it, so this family overrides none.

use super::DecisionFamily;

/// The family of the `branch_outcome` question kind.
pub struct BranchOutcomeFamily;

impl DecisionFamily for BranchOutcomeFamily {}
