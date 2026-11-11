//! The `item_recommendation` question family (MEDULLA-DECISION-ENGINE-R001.101).
//!
//! Ranks through the shared matrix ladder; the registry's default hooks
//! carry it, so this family overrides none.

use super::DecisionFamily;

/// The family of the `item_recommendation` question kind.
pub struct ItemRecommendationFamily;

impl DecisionFamily for ItemRecommendationFamily {}
