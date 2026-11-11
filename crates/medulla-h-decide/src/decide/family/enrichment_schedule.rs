//! The `enrichment_schedule` question family (MEDULLA-DECISION-ENGINE-R001.101).
//!
//! Declared candidates may not declare a negative `enrichment.cost_q32`
//! (MEDULLA-DECISION-ENGINE-R031); the gate runs at the candidates stage.

use medulla_decision_types::decision::statistical::declared::DeclaredOption;
use medulla_decision_types::decision::statistical::enrichment_schedule::{
    EnrichmentCandidate, EnrichmentScheduleRequest, COST_KEY, EXPECTED_VALUE_KEY,
};
use medulla_decision_types::decision::statistical::StatisticalErrorCode;

use super::DecisionFamily;

/// The family of the `enrichment_schedule` question kind.
pub struct EnrichmentScheduleFamily;

impl DecisionFamily for EnrichmentScheduleFamily {
    fn validate_candidates(
        &self,
        options: &[DeclaredOption],
        _principal: &str,
    ) -> Result<(), String> {
        let candidates = options
            .iter()
            .map(|option| EnrichmentCandidate {
                work_id: option.option_id.clone(),
                expected_value_q32: crate::decide::candidates::declared_number(
                    option,
                    EXPECTED_VALUE_KEY,
                ),
                cost_q32: crate::decide::candidates::declared_number(option, COST_KEY),
            })
            .collect();
        EnrichmentScheduleRequest { candidates }
            .check()
            .map_err(|detail| {
                crate::decide::candidates::prefixed_refusal(
                    &detail,
                    "UNSUPPORTED_ENRICHMENT_COST: ",
                    StatisticalErrorCode::UnsupportedEnrichmentCost,
                )
            })
    }
}
