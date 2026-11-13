//! Delivery records (MEDULLA-SDLC-R019.2).
//!
//! A delivery is one governed attempt to land a slice of requirement rows. It
//! carries its mandate, the runs assigned to it, the proposals it opened or
//! follows and how it ended. Every closed set is a serde enum, so an unknown
//! value never decodes; [`decode_delivery`] maps that to the stable refusal
//! `DELIVERY_RECORD_INVALID`. Every record carries `schema_version`.

use serde::{Deserialize, Serialize};

/// The schema version every delivery record carries.
pub const DELIVERY_SCHEMA_VERSION: u32 = 1;

/// The refusal code for a delivery record that does not decode.
pub const DELIVERY_RECORD_INVALID: &str = "DELIVERY_RECORD_INVALID";

/// How strongly the delivery was mandated.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MandateStrength {
    Explicit,
    Inferred,
}

/// Lifecycle state of one assignment: pending -> running -> ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssignmentState {
    Pending,
    Running,
    Ended,
}

impl AssignmentState {
    /// Whether `self -> next` is a legal transition of the assignment machine.
    pub fn can_move_to(self, next: Self) -> bool {
        matches!(
            (self, next),
            (Self::Pending, Self::Running)
                | (Self::Pending, Self::Ended)
                | (Self::Running, Self::Ended)
        )
    }
}

/// The last report state an assignment declared.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryReportState {
    Progress,
    Waiting,
    Ready,
    Blocked,
    Failed,
    Dropped,
}

/// How a delivery ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryOutcome {
    Completed,
    Merged,
    Closed,
    Dropped,
    Stopped,
}

/// Feedback event kinds; mirrors `ScmFeedbackEvent.kind` of SCION-SCM-R003.1.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FeedbackKind {
    CheckChanged,
    ReviewSubmitted,
    ReviewComment,
    ConversationComment,
    ProposalMerged,
    ProposalClosed,
    ProposalReopened,
    HeadPushed,
}

/// The merge-arming verdict for a proposal.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArmingVerdict {
    Arm,
    Disarm,
    Hold,
}

/// Who mandated the delivery, on what basis, and over which rows.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeliveryMandate {
    pub schema_version: u32,
    pub strength: MandateStrength,
    pub basis: String,
    pub policy_version: String,
    pub actor: String,
    pub scope_rows: Vec<String>,
    pub project_mode: bool,
}

/// One run assigned to a delivery.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeliveryAssignment {
    pub schema_version: u32,
    pub id: String,
    pub run_trace_id: String,
    pub state: AssignmentState,
    pub report: Option<DeliveryReportState>,
    pub started_at: Option<i64>,
    pub ended_at: Option<i64>,
    pub ended_reason: Option<String>,
}

/// The delivery record.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Delivery {
    pub schema_version: u32,
    pub id: String,
    pub tenant: String,
    pub repo: String,
    pub slice_root: String,
    pub scope_rows: Vec<String>,
    pub mandate: DeliveryMandate,
    pub proposals: Vec<String>,
    pub follows: Vec<String>,
    pub assignments: Vec<DeliveryAssignment>,
    pub inputs: Vec<String>,
    pub attention: Vec<FeedbackKind>,
    pub ended: Option<DeliveryOutcome>,
    pub created_at: i64,
}

/// A delivery record that does not decode.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("DELIVERY_RECORD_INVALID: {0}")]
pub struct DeliveryRecordInvalid(pub String);

impl DeliveryRecordInvalid {
    /// The stable refusal code.
    pub fn code(&self) -> &'static str {
        DELIVERY_RECORD_INVALID
    }
}

/// Decode a delivery record, refusing an unknown value or a wrong schema version.
pub fn decode_delivery(value: serde_json::Value) -> Result<Delivery, DeliveryRecordInvalid> {
    let delivery: Delivery =
        serde_json::from_value(value).map_err(|e| DeliveryRecordInvalid(e.to_string()))?;
    let versions = std::iter::once(delivery.schema_version)
        .chain(std::iter::once(delivery.mandate.schema_version))
        .chain(delivery.assignments.iter().map(|a| a.schema_version));
    for version in versions {
        if version != DELIVERY_SCHEMA_VERSION {
            return Err(DeliveryRecordInvalid(format!("schema_version {version}")));
        }
    }
    Ok(delivery)
}
