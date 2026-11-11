//! The `context_budget` question family (MEDULLA-DECISION-ENGINE-R001.101).
//!
//! Ranks through the shared matrix ladder; the registry's default hooks
//! carry it, so this family overrides none.

use super::DecisionFamily;

/// The family of the `context_budget` question kind.
pub struct ContextBudgetFamily;

impl DecisionFamily for ContextBudgetFamily {}
