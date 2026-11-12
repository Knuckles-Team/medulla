//! The `round_verdict` question family (MEDULLA-DECISION-ENGINE-R001.101).
//!
//! Ranks through the shared matrix ladder; the registry's default hooks
//! carry it, so this family overrides none.

use super::DecisionFamily;

/// The family of the `round_verdict` question kind.
pub struct RoundVerdictFamily;

impl DecisionFamily for RoundVerdictFamily {}
