//! The `usage_guidance` question family (MEDULLA-DECISION-ENGINE-R001.101).
//!
//! Ranks through the shared matrix ladder; the registry's default hooks
//! carry it, so this family overrides none.

use super::DecisionFamily;

/// The family of the `usage_guidance` question kind.
pub struct UsageGuidanceFamily;

impl DecisionFamily for UsageGuidanceFamily {}
