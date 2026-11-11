//! The `template_choice` question family (MEDULLA-DECISION-ENGINE-R001.101).
//!
//! Ranks through the shared matrix ladder; the registry's default hooks
//! carry it, so this family overrides none.

use super::DecisionFamily;

/// The family of the `template_choice` question kind.
pub struct TemplateChoiceFamily;

impl DecisionFamily for TemplateChoiceFamily {}
