//! The `skill_workflow_compile` question family (MEDULLA-DECISION-ENGINE-R156.1).
//!
//! Compiles a skill document's typed draft into a stored program. The model is
//! `medulla_decision_types::decision::skill_workflow`; the compile body and its
//! premises are the rows that follow R156.1.

use super::DecisionFamily;

/// The family of the `skill_workflow_compile` question kind.
pub struct SkillWorkflowFamily;

impl DecisionFamily for SkillWorkflowFamily {}
