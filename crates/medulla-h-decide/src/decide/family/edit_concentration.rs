//! The `edit_concentration` question family (MEDULLA-DECISION-ENGINE-R001.101).
//!
//! Ranks through the shared matrix ladder; the registry's default hooks
//! carry it, so this family overrides none.

use super::DecisionFamily;

/// The family of the `edit_concentration` question kind.
pub struct EditConcentrationFamily;

impl DecisionFamily for EditConcentrationFamily {}
