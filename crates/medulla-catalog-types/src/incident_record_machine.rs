//! MEDULLA-ORCH-VOCAB-R815.6 -- the served incident record's status as one explicit state
//! machine (organization constitution Article XXXI).
//!
//! [`IncidentStatus::apply`] is the one transition function and `IncidentRecord.status` is
//! the one persisted field it writes. A status moves open, acknowledged, resolved; every other
//! move is refused with the typed `TRANSITION_REFUSED` record.

use medulla_types::transition_refused::{
    transition, Edge, MachineDef, MachineEvent, MachineState, StateMachineApply, TransitionRefused,
};

use crate::incident::IncidentStatus;

/// An event of `IncidentRecordMachine`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum IncidentRecordEvent {
    /// An operator acknowledged the incident.
    Acknowledge,
    /// The incident was resolved.
    Resolve,
}

impl MachineState for IncidentStatus {
    fn stored(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Acknowledged => "acknowledged",
            Self::Resolved => "resolved",
        }
    }
}

impl MachineEvent for IncidentRecordEvent {
    fn name(self) -> &'static str {
        match self {
            Self::Acknowledge => "acknowledge",
            Self::Resolve => "resolve",
        }
    }
}

/// `IncidentRecordMachine`: the closed edge table. Every other pair of state and event is
/// refused.
pub static INCIDENT_RECORD_MACHINE: MachineDef<IncidentStatus, IncidentRecordEvent, &'static str> =
    MachineDef {
        name: "IncidentRecordMachine",
        states: &[
            IncidentStatus::Open,
            IncidentStatus::Acknowledged,
            IncidentStatus::Resolved,
        ],
        initial: IncidentStatus::Open,
        terminals: &[IncidentStatus::Resolved],
        events: &[IncidentRecordEvent::Acknowledge, IncidentRecordEvent::Resolve],
        guards: &[],
        edges: &[
            Edge {
                id: "R01",
                from: IncidentStatus::Open,
                event: IncidentRecordEvent::Acknowledge,
                guard: None,
                to: IncidentStatus::Acknowledged,
            },
            Edge {
                id: "R02",
                from: IncidentStatus::Acknowledged,
                event: IncidentRecordEvent::Resolve,
                guard: None,
                to: IncidentStatus::Resolved,
            },
        ],
    };

impl IncidentStatus {
    /// The one transition function of `IncidentRecordMachine`.
    pub fn apply(self, event: IncidentRecordEvent) -> Result<Self, TransitionRefused> {
        transition(&INCIDENT_RECORD_MACHINE, self, event, |_| false).map(|edge| edge.to)
    }

    /// The event that requests status `to`. A requested `Open` has no event: nothing moves
    /// back to the initial state.
    pub fn event_to(to: Self) -> Option<IncidentRecordEvent> {
        match to {
            Self::Open => None,
            Self::Acknowledged => Some(IncidentRecordEvent::Acknowledge),
            Self::Resolved => Some(IncidentRecordEvent::Resolve),
        }
    }
}

impl StateMachineApply for IncidentStatus {
    type Event = IncidentRecordEvent;
    type Facts = ();
    fn apply(&self, event: IncidentRecordEvent, _facts: &()) -> Result<Self, TransitionRefused> {
        IncidentStatus::apply(*self, event)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_types::transition_refused::TransitionRefusalReason;

    // spec: MEDULLA-ORCH-VOCAB-R815.6
    #[test]
    fn every_legal_transition_applies() {
        let machine = &INCIDENT_RECORD_MACHINE;
        assert_eq!(machine.edges.len(), 2);
        for edge in machine.edges {
            assert_eq!(edge.from.apply(edge.event), Ok(edge.to), "{}", edge.id);
        }
        assert_eq!(
            IncidentStatus::event_to(IncidentStatus::Acknowledged),
            Some(IncidentRecordEvent::Acknowledge)
        );
        assert_eq!(IncidentStatus::event_to(IncidentStatus::Open), None);
    }

    // spec: MEDULLA-ORCH-VOCAB-R815.6
    #[test]
    fn every_illegal_pair_is_refused_with_transition_refused() {
        let machine = &INCIDENT_RECORD_MACHINE;
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
                } else {
                    TransitionRefusalReason::IllegalTransition
                };
                assert_eq!(refused.reason, reason);
                assert_eq!(refused.machine, "IncidentRecordMachine");
                assert_eq!(refused.from, state.stored());
                assert_eq!(refused.event, event.name());
                assert_eq!(refused.code().as_str(), "TRANSITION_REFUSED");
            }
        }
        assert_eq!(illegal, 3 * 2 - 2);
    }
}
