//! The `skill_upgrade` question family (MEDULLA-DECISION-ENGINE-R167.1).
//!
//! Whether a new consumer contract still serves the previous one, for a skill,
//! tool or prompt. The request model is
//! `medulla_decision_types::decision::skill_upgrade_request`; the admission body
//! and its premises are the rows that follow R167.1.

use super::DecisionFamily;

/// The family of the `skill_upgrade` question kind.
pub struct SkillUpgradeFamily;

impl DecisionFamily for SkillUpgradeFamily {}
