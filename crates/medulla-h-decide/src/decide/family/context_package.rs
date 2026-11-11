//! The `context_package` question family (MEDULLA-DECISION-ENGINE-R001.101).
//!
//! Ranks through the shared matrix ladder; the registry's default hooks
//! carry it, so this family overrides none.

use super::DecisionFamily;

/// The family of the `context_package` question kind.
pub struct ContextPackageFamily;

impl DecisionFamily for ContextPackageFamily {}
