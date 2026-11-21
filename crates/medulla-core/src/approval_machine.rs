//! MEDULLA-BUSINESS-OPS-R001.7 — the approval request as one explicit state machine
//! (organization constitution Article XXXI).
//!
//! `ApprovalRequestMachine::apply` is the only writer of a request's status. A
//! `Pending` request is approved once the quorum of its approval rule is met, rejected
//! or deferred by an approver who holds the required role. A `Deferred` request is
//! decided later. `Approved` and `Rejected` are terminal.

use medulla_types::transition_refused::{
    transition, Edge, MachineDef, MachineEvent, MachineState, StateMachineApply, TransitionRefused,
};

/// The status of one approval request.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ApprovalStatus {
    /// Awaiting a decision.
    Pending,
    /// The quorum of approvers approved it.
    Approved,
    /// An approver rejected it.
    Rejected,
    /// An approver set it aside for a later decision.
    Deferred,
}

impl MachineState for ApprovalStatus {
    fn stored(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Approved => "approved",
            Self::Rejected => "rejected",
            Self::Deferred => "deferred",
        }
    }
}

/// An event of `ApprovalRequestMachine`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ApprovalEvent {
    /// The `Approve` event.
    Approve,
    /// The `Reject` event.
    Reject,
    /// The `Defer` event.
    Defer,
}

impl MachineEvent for ApprovalEvent {
    fn name(self) -> &'static str {
        match self {
            Self::Approve => "Approve",
            Self::Reject => "Reject",
            Self::Defer => "Defer",
        }
    }
}

/// The facts the approval guards read.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ApprovalFacts {
    /// The deciding approver holds the required role (guard of A02, A03 and A05).
    pub approver_holds_role: bool,
    /// The approvals recorded reach the rule's quorum (guard of A01 and A04).
    pub quorum_met: bool,
}

/// Guard of `ApprovalRequestMachine`: the approver holds the required role.
pub const APPROVAL_GUARD_APPROVER_HOLDS_ROLE: &str = "ApproverHoldsRole";
/// Guard of `ApprovalRequestMachine`: the approvals reach the quorum.
pub const APPROVAL_GUARD_QUORUM_MET: &str = "QuorumMet";

/// `ApprovalRequestMachine`: the closed edge table. Every other pair of state and event
/// is refused.
pub static APPROVAL_REQUEST_MACHINE: MachineDef<ApprovalStatus, ApprovalEvent, &'static str> =
    MachineDef {
        name: "ApprovalRequestMachine",
        states: &[
            ApprovalStatus::Pending,
            ApprovalStatus::Approved,
            ApprovalStatus::Rejected,
            ApprovalStatus::Deferred,
        ],
        initial: ApprovalStatus::Pending,
        terminals: &[ApprovalStatus::Approved, ApprovalStatus::Rejected],
        events: &[
            ApprovalEvent::Approve,
            ApprovalEvent::Reject,
            ApprovalEvent::Defer,
        ],
        guards: &[
            APPROVAL_GUARD_APPROVER_HOLDS_ROLE,
            APPROVAL_GUARD_QUORUM_MET,
        ],
        edges: &[
            Edge {
                id: "A01",
                from: ApprovalStatus::Pending,
                event: ApprovalEvent::Approve,
                guard: Some(APPROVAL_GUARD_QUORUM_MET),
                to: ApprovalStatus::Approved,
            },
            Edge {
                id: "A02",
                from: ApprovalStatus::Pending,
                event: ApprovalEvent::Reject,
                guard: Some(APPROVAL_GUARD_APPROVER_HOLDS_ROLE),
                to: ApprovalStatus::Rejected,
            },
            Edge {
                id: "A03",
                from: ApprovalStatus::Pending,
                event: ApprovalEvent::Defer,
                guard: Some(APPROVAL_GUARD_APPROVER_HOLDS_ROLE),
                to: ApprovalStatus::Deferred,
            },
            Edge {
                id: "A04",
                from: ApprovalStatus::Deferred,
                event: ApprovalEvent::Approve,
                guard: Some(APPROVAL_GUARD_QUORUM_MET),
                to: ApprovalStatus::Approved,
            },
            Edge {
                id: "A05",
                from: ApprovalStatus::Deferred,
                event: ApprovalEvent::Reject,
                guard: Some(APPROVAL_GUARD_APPROVER_HOLDS_ROLE),
                to: ApprovalStatus::Rejected,
            },
        ],
    };

/// The approval request machine: its one transition function.
pub struct ApprovalRequestMachine;

impl ApprovalRequestMachine {
    /// The one transition function of `ApprovalRequestMachine`. The guards read `facts`.
    pub fn apply(
        status: ApprovalStatus,
        event: ApprovalEvent,
        facts: ApprovalFacts,
    ) -> Result<ApprovalStatus, TransitionRefused> {
        transition(&APPROVAL_REQUEST_MACHINE, status, event, |guard| match guard {
            APPROVAL_GUARD_APPROVER_HOLDS_ROLE => facts.approver_holds_role,
            APPROVAL_GUARD_QUORUM_MET => facts.quorum_met,
            _ => false,
        })
        .map(|edge| edge.to)
    }
}

impl StateMachineApply for ApprovalStatus {
    type Event = ApprovalEvent;
    type Facts = ApprovalFacts;
    fn apply(&self, event: ApprovalEvent, facts: &ApprovalFacts) -> Result<Self, TransitionRefused> {
        ApprovalRequestMachine::apply(*self, event, *facts)
    }
}
