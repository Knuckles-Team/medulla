//! MEDULLA-BUSINESS-OPS-R001.5 — the compliance obligation instance as one explicit
//! state machine (organization constitution Article XXXI).
//!
//! `ObligationInstanceMachine::apply` is the only writer of an instance's status:
//! `Open` moves to `Satisfied` or `Waived` on recorded evidence or a recorded waiver,
//! or to `Overdue` once its due date has passed; `Overdue` still closes by evidence or
//! waiver. `Satisfied` and `Waived` are terminal.

use medulla_types::transition_refused::{
    transition, Edge, MachineDef, MachineEvent, MachineState, StateMachineApply, TransitionRefused,
};

/// The status of one obligation instance.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ObligationStatus {
    /// Not yet satisfied, waived or overdue.
    Open,
    /// Evidence of compliance is recorded.
    Satisfied,
    /// A waiver is recorded.
    Waived,
    /// The due date passed with the instance still open.
    Overdue,
}

impl MachineState for ObligationStatus {
    fn stored(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Satisfied => "satisfied",
            Self::Waived => "waived",
            Self::Overdue => "overdue",
        }
    }
}

/// An event of `ObligationInstanceMachine`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ObligationEvent {
    /// The `Satisfy` event.
    Satisfy,
    /// The `Waive` event.
    Waive,
    /// The `MarkOverdue` event.
    MarkOverdue,
}

impl MachineEvent for ObligationEvent {
    fn name(self) -> &'static str {
        match self {
            Self::Satisfy => "Satisfy",
            Self::Waive => "Waive",
            Self::MarkOverdue => "MarkOverdue",
        }
    }
}

/// The facts the obligation guards read.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ObligationFacts {
    /// An evidence item is recorded (guard of O01 and O04).
    pub evidence_recorded: bool,
    /// A waiver is recorded (guard of O02 and O05).
    pub waiver_recorded: bool,
    /// The as-of time is past the due date (guard of O03).
    pub due_date_passed: bool,
}

/// Guard of `ObligationInstanceMachine`: an evidence item is recorded.
pub const OBLIGATION_GUARD_EVIDENCE_RECORDED: &str = "EvidenceRecorded";
/// Guard of `ObligationInstanceMachine`: a waiver is recorded.
pub const OBLIGATION_GUARD_WAIVER_RECORDED: &str = "WaiverRecorded";
/// Guard of `ObligationInstanceMachine`: the due date has passed.
pub const OBLIGATION_GUARD_DUE_DATE_PASSED: &str = "DueDatePassed";

/// `ObligationInstanceMachine`: the closed edge table. Every other pair of state and
/// event is refused.
pub static OBLIGATION_INSTANCE_MACHINE: MachineDef<
    ObligationStatus,
    ObligationEvent,
    &'static str,
> = MachineDef {
    name: "ObligationInstanceMachine",
    states: &[
        ObligationStatus::Open,
        ObligationStatus::Satisfied,
        ObligationStatus::Waived,
        ObligationStatus::Overdue,
    ],
    initial: ObligationStatus::Open,
    terminals: &[ObligationStatus::Satisfied, ObligationStatus::Waived],
    events: &[
        ObligationEvent::Satisfy,
        ObligationEvent::Waive,
        ObligationEvent::MarkOverdue,
    ],
    guards: &[
        OBLIGATION_GUARD_EVIDENCE_RECORDED,
        OBLIGATION_GUARD_WAIVER_RECORDED,
        OBLIGATION_GUARD_DUE_DATE_PASSED,
    ],
    edges: &[
        Edge {
            id: "O01",
            from: ObligationStatus::Open,
            event: ObligationEvent::Satisfy,
            guard: Some(OBLIGATION_GUARD_EVIDENCE_RECORDED),
            to: ObligationStatus::Satisfied,
        },
        Edge {
            id: "O02",
            from: ObligationStatus::Open,
            event: ObligationEvent::Waive,
            guard: Some(OBLIGATION_GUARD_WAIVER_RECORDED),
            to: ObligationStatus::Waived,
        },
        Edge {
            id: "O03",
            from: ObligationStatus::Open,
            event: ObligationEvent::MarkOverdue,
            guard: Some(OBLIGATION_GUARD_DUE_DATE_PASSED),
            to: ObligationStatus::Overdue,
        },
        Edge {
            id: "O04",
            from: ObligationStatus::Overdue,
            event: ObligationEvent::Satisfy,
            guard: Some(OBLIGATION_GUARD_EVIDENCE_RECORDED),
            to: ObligationStatus::Satisfied,
        },
        Edge {
            id: "O05",
            from: ObligationStatus::Overdue,
            event: ObligationEvent::Waive,
            guard: Some(OBLIGATION_GUARD_WAIVER_RECORDED),
            to: ObligationStatus::Waived,
        },
    ],
};

/// The obligation instance machine: its one transition function.
pub struct ObligationInstanceMachine;

impl ObligationInstanceMachine {
    /// The one transition function of `ObligationInstanceMachine`. The guards read
    /// `facts`.
    pub fn apply(
        status: ObligationStatus,
        event: ObligationEvent,
        facts: ObligationFacts,
    ) -> Result<ObligationStatus, TransitionRefused> {
        transition(
            &OBLIGATION_INSTANCE_MACHINE,
            status,
            event,
            |guard| match guard {
                OBLIGATION_GUARD_EVIDENCE_RECORDED => facts.evidence_recorded,
                OBLIGATION_GUARD_WAIVER_RECORDED => facts.waiver_recorded,
                OBLIGATION_GUARD_DUE_DATE_PASSED => facts.due_date_passed,
                _ => false,
            },
        )
        .map(|edge| edge.to)
    }
}

impl StateMachineApply for ObligationStatus {
    type Event = ObligationEvent;
    type Facts = ObligationFacts;
    fn apply(
        &self,
        event: ObligationEvent,
        facts: &ObligationFacts,
    ) -> Result<Self, TransitionRefused> {
        ObligationInstanceMachine::apply(*self, event, *facts)
    }
}
