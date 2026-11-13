//! The `capability_designation` question family (MEDULLA-DECISION-ENGINE-R172.1).
//!
//! Which agents, tools or skills are best able to do a job. The model is
//! `medulla_decision_types::decision::capability_designation`; the filter,
//! scoring and evidence are the rows that follow R172.1.

use super::DecisionFamily;

/// The family of the `capability_designation` question kind.
pub struct CapabilityDesignationFamily;

impl DecisionFamily for CapabilityDesignationFamily {}
