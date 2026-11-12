//! The `template_choice` question family (MEDULLA-DECISION-ENGINE-R001.101).
//!
//! Ranks through the shared matrix ladder; the registry's default hooks
//! carry it. It overrides one hook: it binds the chosen template to the
//! utterance (MEDULLA-DECISION-ENGINE-R001.101.3).

use super::DecisionFamily;

/// The family of the `template_choice` question kind.
pub struct TemplateChoiceFamily;

impl DecisionFamily for TemplateChoiceFamily {
    fn binds_utterance(&self) -> bool {
        true
    }
}
