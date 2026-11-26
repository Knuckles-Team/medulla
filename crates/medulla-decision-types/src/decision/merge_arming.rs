//! The `merge_arming` question family (MEDULLA-SDLC-R019.10).
//!
//! The question names the delivery, the proposal and the head the caller sees now. The engine
//! reads every other fact from the graph the caller can see: the delivery record, the landing
//! policy of the branch, the sealed gate verdicts of the head changeset, the owner approval
//! records and the reviews of the proposal. The verdict is `arm`, `disarm` or `hold`; the engine
//! never merges. A missing owner approval, or one that names another commit than the head,
//! abstains with the registered refusal code.

use medulla_source_types::delivery::{ArmingRefusal, ArmingVerdict};

use super::record::outcome::{DecisionRefusal, RefusalCode};

/// Text parameter: the graph holding the delivery and its records.
pub const PARAM_GRAPH: &str = "graph";
/// Text parameter: the node id of the delivery.
pub const PARAM_DELIVERY: &str = "delivery_id";
/// Text parameter: the proposal the verdict is for.
pub const PARAM_PROPOSAL: &str = "proposal_id";
/// Text parameter: the head commit digest now.
pub const PARAM_HEAD_COMMIT: &str = "head_commit";
/// Text parameter: the id of the head changeset the gate verdicts judged.
pub const PARAM_HEAD_CHANGESET: &str = "head_changeset";
/// Text parameter: the branch whose landing policy lists the required gates.
pub const PARAM_BRANCH: &str = "branch";
/// Text parameter: the gate id of the required check.
pub const PARAM_REQUIRED_CHECK: &str = "required_check";
/// Integer parameter: the changed lines of the proposal.
pub const PARAM_CHANGED_LINES: &str = "changed_lines";

/// The node type of an owner approval record.
pub const OWNER_APPROVAL_TYPE: &str = "OwnerApproval";
/// The node type of a review record.
pub const REVIEW_TYPE: &str = "Review";
/// Row key of an approval or review: the proposal it concerns.
pub const ROW_PROPOSAL: &str = "proposal_id";
/// Row key of an approval or review: the head commit digest it names.
pub const ROW_HEAD_COMMIT: &str = "head_commit";
/// Row key of an approval or review: when it was recorded, in milliseconds.
pub const ROW_AT_MS: &str = "recorded_at_ms";
/// Row key of a review: who wrote it.
pub const ROW_REVIEWER: &str = "reviewer";
/// Row key of a review: `approved`, `changes_requested` or `commented`.
pub const ROW_REVIEW_STATE: &str = "state";
/// The review state that disarms.
pub const STATE_CHANGES_REQUESTED: &str = "changes_requested";

/// Index of the knob `sdlc.delivery.arm_max_changed_lines` in
/// [`super::lifecycle::KNOBS`].
pub const KNOB_ARM_MAX_CHANGED_LINES: usize = 4;

/// The refusal table of the family.
pub const REFUSALS: &[RefusalCode] = &[
    RefusalCode::new(medulla_source_types::delivery::ARMING_APPROVAL_MISSING),
    RefusalCode::new(medulla_source_types::delivery::ARMING_HEAD_MISMATCH),
];

/// The option id an answer names its verdict with.
pub fn option_id(verdict: ArmingVerdict) -> &'static str {
    match verdict {
        ArmingVerdict::Arm => "verdict:arm",
        ArmingVerdict::Disarm => "verdict:disarm",
        ArmingVerdict::Hold => "verdict:hold",
    }
}

/// A refused verdict, as the abstention reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MergeArmingRefusal(pub ArmingRefusal);

impl DecisionRefusal for MergeArmingRefusal {
    fn code(&self) -> RefusalCode {
        match self.0 {
            ArmingRefusal::ApprovalMissing(_) => REFUSALS[0],
            ArmingRefusal::HeadMismatch(_) => REFUSALS[1],
        }
    }

    fn detail(&self) -> String {
        match &self.0 {
            ArmingRefusal::ApprovalMissing(detail) | ArmingRefusal::HeadMismatch(detail) => {
                detail.clone()
            }
        }
    }
}
