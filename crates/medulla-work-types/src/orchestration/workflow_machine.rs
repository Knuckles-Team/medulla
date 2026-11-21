//! MEDULLA-ORCH-VOCAB-R815.9 -- the registered workflow's lifecycle as one explicit state
//! machine (organization constitution Article XXXI).
//!
//! [`WorkflowLifecycle::apply`] is the one transition function and
//! `WorkflowRegistrationEntry.lifecycle` is the one persisted field it writes. `Reactivate`
//! has no edge: the identity carries its content digest, so a stale or withdrawn version never
//! becomes active again, and a new version is a new identity.

use medulla_types::transition_refused::{
    transition, Edge, MachineDef, MachineEvent, MachineState, StateMachineApply, TransitionRefused,
};

use super::workflows::WorkflowLifecycle;

/// An event of `WorkflowMachine`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum WorkflowEvent {
    /// The registration went stale.
    MarkStale,
    /// The registration was withdrawn.
    Withdraw,
    /// An inactive registration was asked to become active again; no edge accepts it.
    Reactivate,
}

impl MachineState for WorkflowLifecycle {
    fn stored(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Stale => "stale",
            Self::Withdrawn => "withdrawn",
        }
    }
}

impl MachineEvent for WorkflowEvent {
    fn name(self) -> &'static str {
        match self {
            Self::MarkStale => "mark_stale",
            Self::Withdraw => "withdraw",
            Self::Reactivate => "reactivate",
        }
    }
}

/// `WorkflowMachine`: the closed edge table. Every other pair of state and event is refused.
pub static WORKFLOW_MACHINE: MachineDef<WorkflowLifecycle, WorkflowEvent, &'static str> =
    MachineDef {
        name: "WorkflowMachine",
        states: &[
            WorkflowLifecycle::Active,
            WorkflowLifecycle::Stale,
            WorkflowLifecycle::Withdrawn,
        ],
        initial: WorkflowLifecycle::Active,
        terminals: &[WorkflowLifecycle::Withdrawn],
        events: &[
            WorkflowEvent::MarkStale,
            WorkflowEvent::Withdraw,
            WorkflowEvent::Reactivate,
        ],
        guards: &[],
        edges: &[
            Edge {
                id: "F01",
                from: WorkflowLifecycle::Active,
                event: WorkflowEvent::MarkStale,
                guard: None,
                to: WorkflowLifecycle::Stale,
            },
            Edge {
                id: "F02",
                from: WorkflowLifecycle::Active,
                event: WorkflowEvent::Withdraw,
                guard: None,
                to: WorkflowLifecycle::Withdrawn,
            },
            Edge {
                id: "F03",
                from: WorkflowLifecycle::Stale,
                event: WorkflowEvent::Withdraw,
                guard: None,
                to: WorkflowLifecycle::Withdrawn,
            },
        ],
    };

impl WorkflowLifecycle {
    /// The one transition function of `WorkflowMachine`.
    pub fn apply(self, event: WorkflowEvent) -> Result<Self, TransitionRefused> {
        transition(&WORKFLOW_MACHINE, self, event, |_| false).map(|edge| edge.to)
    }

    /// The event that requests moving to `to`.
    pub fn event_to(to: Self) -> WorkflowEvent {
        match to {
            Self::Stale => WorkflowEvent::MarkStale,
            Self::Withdrawn => WorkflowEvent::Withdraw,
            Self::Active => WorkflowEvent::Reactivate,
        }
    }
}

impl StateMachineApply for WorkflowLifecycle {
    type Event = WorkflowEvent;
    type Facts = ();
    fn apply(&self, event: WorkflowEvent, _facts: &()) -> Result<Self, TransitionRefused> {
        WorkflowLifecycle::apply(*self, event)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_types::transition_refused::TransitionRefusalReason;

    // spec: MEDULLA-ORCH-VOCAB-R815.9
    #[test]
    fn every_legal_transition_applies() {
        let machine = &WORKFLOW_MACHINE;
        assert_eq!(machine.edges.len(), 3);
        for edge in machine.edges {
            assert_eq!(edge.from.apply(edge.event), Ok(edge.to), "{}", edge.id);
        }
        assert_eq!(
            WORKFLOW_MACHINE.state_from_stored("stale"),
            Some(WorkflowLifecycle::Stale)
        );
        assert_eq!(WORKFLOW_MACHINE.state_from_stored("gone"), None);
    }

    // spec: MEDULLA-ORCH-VOCAB-R815.9
    #[test]
    fn every_illegal_pair_is_refused_with_transition_refused() {
        let machine = &WORKFLOW_MACHINE;
        let mut illegal = 0;
        for &state in machine.states {
            for &event in machine.events {
                if machine
                    .edges
                    .iter()
                    .any(|e| e.from == state && e.event == event)
                {
                    continue;
                }
                illegal += 1;
                let refused = state.apply(event).unwrap_err();
                let reason = if machine.is_terminal(state) {
                    TransitionRefusalReason::TerminalState
                } else if event == WorkflowEvent::Reactivate {
                    TransitionRefusalReason::UnknownEvent
                } else {
                    TransitionRefusalReason::IllegalTransition
                };
                assert_eq!(refused.reason, reason, "{state:?} {event:?}");
                assert_eq!(refused.machine, "WorkflowMachine");
                assert_eq!(refused.from, state.stored());
                assert_eq!(refused.event, event.name());
                assert_eq!(refused.code().as_str(), "TRANSITION_REFUSED");
            }
        }
        assert_eq!(illegal, 3 * 3 - 3);
    }
}
