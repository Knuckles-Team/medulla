//! The `ingestion_lane` question family (MEDULLA-DECISION-ENGINE-R001.101).
//!
//! Declared candidates must each name a registered ingestion lane
//! (MEDULLA-DECISION-ENGINE-R030); the gate runs at the candidates stage.

use medulla_decision_types::decision::statistical::declared::DeclaredOption;
use medulla_decision_types::decision::statistical::ingestion_lane::IngestionLaneRequest;
use medulla_decision_types::decision::statistical::StatisticalErrorCode;

use super::DecisionFamily;

/// The family of the `ingestion_lane` question kind.
pub struct IngestionLaneFamily;

impl DecisionFamily for IngestionLaneFamily {
    fn validate_candidates(
        &self,
        options: &[DeclaredOption],
        _principal: &str,
    ) -> Result<(), String> {
        IngestionLaneRequest {
            candidate_lanes: options.iter().map(|o| o.option_id.clone()).collect(),
        }
        .check()
        .map_err(|detail| {
            crate::decide::candidates::prefixed_refusal(
                &detail,
                "UNSUPPORTED_INGESTION_LANE: ",
                StatisticalErrorCode::UnsupportedIngestionLane,
            )
        })
    }
}
