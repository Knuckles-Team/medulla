//! The `resolve_entity` question family (MEDULLA-DECISION-ENGINE-R001.101).
//!
//! Ranks through the shared matrix ladder; the registry's default hooks
//! carry it, so this family overrides none.

use super::DecisionFamily;

/// The family of the `resolve_entity` question kind.
pub struct ResolveEntityFamily;

impl DecisionFamily for ResolveEntityFamily {}
