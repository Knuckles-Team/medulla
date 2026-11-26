//! Delivery records (MEDULLA-SDLC-R019.2).
//!
//! A delivery is one governed attempt to land a slice of requirement rows. It
//! carries its mandate, the runs assigned to it, the proposals it opened or
//! follows and how it ended. Every closed set is a serde enum, so an unknown
//! value never decodes; [`decode_delivery`] maps that to the stable refusal
//! `DELIVERY_RECORD_INVALID`. Every record carries `schema_version`.

use medulla_types::refusal_code::RefusalCode;
use medulla_types::transition_refused::{
    transition, Edge, MachineDef, MachineEvent, MachineState, TransitionRefused,
};
use serde::{Deserialize, Serialize};

/// The schema version every delivery record carries.
pub const DELIVERY_SCHEMA_VERSION: u32 = 1;

/// The refusal code for a delivery record that does not decode.
pub const DELIVERY_RECORD_INVALID: &str = "DELIVERY_RECORD_INVALID";

/// The refusal code for a report naming an assignment the delivery does not hold (R019.7).
pub const ASSIGNMENT_UNKNOWN: &str = "ASSIGNMENT_UNKNOWN";

/// The refusal code for a report on an assignment that already ended (R019.7).
pub const ASSIGNMENT_ENDED: &str = "ASSIGNMENT_ENDED";

/// The refusal code for a `ready` report without evidence a verdict backs (R019.7).
pub const REPORT_EVIDENCE_MISSING: &str = "REPORT_EVIDENCE_MISSING";

/// The refusal code for a question longer than [`QUESTION_MAX_CHARS`] (R019.8).
pub const QUESTION_TOO_LONG: &str = "QUESTION_TOO_LONG";

/// The longest question a delivery holds, in characters.
pub const QUESTION_MAX_CHARS: usize = 600;

/// No registered connector provides the feedback event (R019.5).
pub const FEEDBACK_SOURCE_UNKNOWN: &str = "FEEDBACK_SOURCE_UNKNOWN";

/// The feedback event names a proposal the engine does not hold (R019.5).
pub const FEEDBACK_PROPOSAL_UNKNOWN: &str = "FEEDBACK_PROPOSAL_UNKNOWN";

/// The feedback event is blank in a required field (R019.5).
pub const FEEDBACK_EVENT_INVALID: &str = "FEEDBACK_EVENT_INVALID";

/// A resume of an ended delivery failed a fact of its mandate (R019.6).
pub const MANDATE_REVOKED: &str = "MANDATE_REVOKED";

/// Every refusal code of the delivery records.
pub const REFUSALS: &[RefusalCode] = &[
    RefusalCode::new(MANDATE_REVOKED),
    RefusalCode::new(FEEDBACK_SOURCE_UNKNOWN),
    RefusalCode::new(FEEDBACK_PROPOSAL_UNKNOWN),
    RefusalCode::new(FEEDBACK_EVENT_INVALID),
    RefusalCode::new(DELIVERY_RECORD_INVALID),
    RefusalCode::new(ASSIGNMENT_UNKNOWN),
    RefusalCode::new(ASSIGNMENT_ENDED),
    RefusalCode::new(REPORT_EVIDENCE_MISSING),
    RefusalCode::new(QUESTION_TOO_LONG),
];

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

/// The latest report of one assignment (R019.7).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssignmentReport {
    pub state: DeliveryReportState,
    pub summary: String,
    pub proposal: Option<String>,
    pub evidence: Vec<String>,
    pub seq: u64,
}

/// Who produced a delivery input.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputSource {
    Person,
    Automation,
}

/// One feedback input a delivery received (R019.7).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeliveryInput {
    pub event_id: String,
    pub kind: FeedbackKind,
    pub source: InputSource,
    pub seq: u64,
    /// What the input names, when the kind needs it (`policy-changed`: the changed fact).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// The one open question of a delivery (R019.7).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeliveryQuestion {
    pub text: String,
    /// The `event_id` of the input that raised the question, when one did.
    pub raised_by: Option<String>,
    pub seq: u64,
}

/// The state of one proposal a delivery opened.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProposalState {
    Open,
    Merged,
    Closed,
}

/// An event of `DeliveryProposalMachine`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ProposalEvent {
    Merged,
    Closed,
    Reopened,
}

impl MachineState for ProposalState {
    fn stored(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Merged => "merged",
            Self::Closed => "closed",
        }
    }
}

impl MachineEvent for ProposalEvent {
    fn name(self) -> &'static str {
        match self {
            Self::Merged => "Merged",
            Self::Closed => "Closed",
            Self::Reopened => "Reopened",
        }
    }
}

/// `DeliveryProposalMachine`: the closed edge table. Every other pair is refused.
pub static DELIVERY_PROPOSAL_MACHINE: MachineDef<ProposalState, ProposalEvent, &'static str> =
    MachineDef {
        name: "DeliveryProposalMachine",
        states: &[
            ProposalState::Open,
            ProposalState::Merged,
            ProposalState::Closed,
        ],
        initial: ProposalState::Open,
        terminals: &[ProposalState::Merged],
        events: &[
            ProposalEvent::Merged,
            ProposalEvent::Closed,
            ProposalEvent::Reopened,
        ],
        guards: &[],
        edges: &[
            Edge {
                id: "DP01",
                from: ProposalState::Open,
                event: ProposalEvent::Merged,
                guard: None,
                to: ProposalState::Merged,
            },
            Edge {
                id: "DP02",
                from: ProposalState::Open,
                event: ProposalEvent::Closed,
                guard: None,
                to: ProposalState::Closed,
            },
            Edge {
                id: "DP03",
                from: ProposalState::Closed,
                event: ProposalEvent::Reopened,
                guard: None,
                to: ProposalState::Open,
            },
        ],
    };

impl ProposalState {
    /// The one transition function of `DeliveryProposalMachine`.
    pub fn apply(self, event: ProposalEvent) -> Result<Self, TransitionRefused> {
        transition(&DELIVERY_PROPOSAL_MACHINE, self, event, |_| false).map(|edge| edge.to)
    }
}

/// One proposal a delivery opened and its state.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeliveryProposal {
    pub id: String,
    pub state: ProposalState,
}

/// The stage of a delivery, derived on each read and never stored (R019.7).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeliveryStage {
    #[serde(rename = "ended")]
    Ended,
    #[serde(rename = "person/answer")]
    PersonAnswer,
    #[serde(rename = "person/review")]
    PersonReview,
    #[serde(rename = "person/blocked")]
    PersonBlocked,
    #[serde(rename = "person/failed")]
    PersonFailed,
    #[serde(rename = "working/queued")]
    WorkingQueued,
    #[serde(rename = "idle")]
    Idle,
    #[serde(rename = "working")]
    Working,
}

impl DeliveryStage {
    /// The wire name of the stage.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ended => "ended",
            Self::PersonAnswer => "person/answer",
            Self::PersonReview => "person/review",
            Self::PersonBlocked => "person/blocked",
            Self::PersonFailed => "person/failed",
            Self::WorkingQueued => "working/queued",
            Self::Idle => "idle",
            Self::Working => "working",
        }
    }
}

/// One report an agent writes to a delivery through `DeliveryReport` (R019.7).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeliveryReportWrite {
    pub delivery_id: String,
    pub assignment_id: String,
    pub state: DeliveryReportState,
    pub summary: String,
    pub proposal: Option<String>,
    pub question: Option<String>,
    pub evidence: Vec<String>,
}

/// A refused report write.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum DeliveryReportRefusal {
    /// The assignment ID names no assignment of the delivery.
    #[error("ASSIGNMENT_UNKNOWN: {0}")]
    AssignmentUnknown(String),
    /// The assignment already ended.
    #[error("ASSIGNMENT_ENDED: {0}")]
    AssignmentEnded(String),
    /// A `ready` report without evidence a known verdict backs.
    #[error("REPORT_EVIDENCE_MISSING: {0}")]
    EvidenceMissing(String),
    /// The question is longer than [`QUESTION_MAX_CHARS`] characters.
    #[error("QUESTION_TOO_LONG: {0} characters")]
    QuestionTooLong(usize),
}

impl DeliveryReportRefusal {
    /// The stable refusal code.
    pub fn code(&self) -> &'static str {
        match self {
            Self::AssignmentUnknown(_) => ASSIGNMENT_UNKNOWN,
            Self::AssignmentEnded(_) => ASSIGNMENT_ENDED,
            Self::EvidenceMissing(_) => REPORT_EVIDENCE_MISSING,
            Self::QuestionTooLong(_) => QUESTION_TOO_LONG,
        }
    }
}

impl Delivery {
    /// Take the next sequence number: add one to `seq` and return the new value.
    pub fn next_seq(&mut self) -> u64 {
        self.seq += 1;
        self.seq
    }
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
    /// A fact of the mandate changed under a live delivery (R019.6); the input's `detail`
    /// names the fact.
    PolicyChanged,
}

/// One source-control feedback event as the delivery feedback route reads it (R019.5): the
/// provider event id the engine dedups by, the connector that provided it, what happened, who
/// caused it, the proposal it concerns and whether a check it reports failed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScmFeedbackEvent {
    pub event_id: String,
    pub connector: String,
    pub kind: FeedbackKind,
    pub source: InputSource,
    pub proposal_id: String,
    /// True for a `check_changed` event whose check failed.
    #[serde(default)]
    pub failed: bool,
    pub summary: String,
    pub observed_at: i64,
}

/// A refused feedback event.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum FeedbackRefusal {
    /// No registered connector provides the event.
    #[error("FEEDBACK_SOURCE_UNKNOWN: {0}")]
    SourceUnknown(String),
    /// The proposal is not one the engine holds.
    #[error("FEEDBACK_PROPOSAL_UNKNOWN: {0}")]
    ProposalUnknown(String),
    /// A required field is blank.
    #[error("FEEDBACK_EVENT_INVALID: {0}")]
    EventInvalid(String),
}

impl FeedbackRefusal {
    /// The stable refusal code.
    pub fn code(&self) -> &'static str {
        match self {
            Self::SourceUnknown(_) => FEEDBACK_SOURCE_UNKNOWN,
            Self::ProposalUnknown(_) => FEEDBACK_PROPOSAL_UNKNOWN,
            Self::EventInvalid(_) => FEEDBACK_EVENT_INVALID,
        }
    }
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
    /// The project mode the delivery was opened under (MEDULLA-SDLC-R012.8, R019.3).
    pub project_mode: crate::lifecycle::ProjectMode,
}

/// One run assigned to a delivery.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeliveryAssignment {
    pub schema_version: u32,
    pub id: String,
    pub run_trace_id: String,
    pub state: AssignmentState,
    pub report: Option<AssignmentReport>,
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
    /// The last sequence number the delivery issued.
    pub seq: u64,
    pub proposals: Vec<DeliveryProposal>,
    pub follows: Vec<String>,
    pub assignments: Vec<DeliveryAssignment>,
    pub inputs: Vec<DeliveryInput>,
    pub attention: Option<DeliveryQuestion>,
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

/// The facts a resume of an ended delivery is checked against (R019.6).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MandateFacts {
    /// The mode of the codebase now.
    pub mode: crate::lifecycle::ProjectMode,
    /// Whether every scope row is still open (none is verified).
    pub rows_open: bool,
    /// Whether the actor still holds `source:write` on the repository, as the admission
    /// stamped on the event.
    pub actor_can_write: bool,
    /// The policy version now.
    pub policy_version: String,
}

/// A refused resume.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("MANDATE_REVOKED: {0}")]
pub struct MandateRevoked(pub String);

impl MandateRevoked {
    /// The stable refusal code.
    pub fn code(&self) -> &'static str {
        MANDATE_REVOKED
    }
}

/// Who produced a review (R019.9).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewerKind {
    Automated,
    Person,
}

/// One stored `:Review` record as the round budget reads it (R019.9).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewRecord {
    pub proposal_id: String,
    pub reviewer: String,
    pub reviewer_kind: ReviewerKind,
    /// The distinct automated review run that produced the review.
    pub run_key: String,
}

/// Whether an automated review is still addressed (R019.9).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoundVerdict {
    Address,
    StopAddressing,
}
