//! The `retrieval_plan` question family (MEDULLA-DECISION-ENGINE-R001.101).
//!
//! A declared candidate claiming the reserved registry namespace must name
//! a registered retrieval plan kind (MEDULLA-DECISION-ENGINE-R029); the gate
//! runs at the candidates stage.

use medulla_decision_types::decision::statistical::declared::DeclaredOption;
use medulla_decision_types::decision::statistical::retrieval_plan::RetrievalPlanKind;
use medulla_decision_types::decision::statistical::StatisticalErrorCode;

use crate::decide::stat_support::refusal;

use super::DecisionFamily;

/// The family of the `retrieval_plan` question kind.
pub struct RetrievalPlanFamily;

impl DecisionFamily for RetrievalPlanFamily {
    fn validate_candidates(
        &self,
        options: &[DeclaredOption],
        _principal: &str,
    ) -> Result<(), String> {
        for option in options {
            if RetrievalPlanKind::claims_registry_namespace(&option.option_id)
                && RetrievalPlanKind::from_option_id(&option.option_id).is_none()
            {
                return Err(refusal(
                    StatisticalErrorCode::ParameterInvalid,
                    format!(
                        "UNSUPPORTED_RETRIEVAL_PLAN: '{}' is not in the retrieval plan \
                         registry (leanrag, reciprocal-rank-fusion, direct-sparql)",
                        option.option_id
                    ),
                ));
            }
        }
        Ok(())
    }
}
