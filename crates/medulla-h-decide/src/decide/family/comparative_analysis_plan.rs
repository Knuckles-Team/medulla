//! The `comparative_analysis_plan` question family (MEDULLA-DECISION-ENGINE-R001.101).
//!
//! Ranks through the shared matrix ladder; the registry's default hooks
//! carry it, so this family overrides none.

use super::DecisionFamily;

/// The family of the `comparative_analysis_plan` question kind.
pub struct ComparativeAnalysisPlanFamily;

impl DecisionFamily for ComparativeAnalysisPlanFamily {}
