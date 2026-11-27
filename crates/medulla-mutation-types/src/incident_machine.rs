//! MEDULLA-ORCH-VOCAB-R815.5 -- the health incident's status as one explicit state machine
//! (organization constitution Article XXXI).
//!
//! [`IncidentStatus::apply`] is the one transition function and `Incident.status` is the one
//! persisted field it writes. A status moves open, acknowledged, resolved, closed; every other
//! move is refused with the typed `TRANSITION_REFUSED` record.

use medulla_types::transition_refused::{
    transition, Edge, MachineDef, MachineEvent, MachineState, StateMachineApply, TransitionRefused,
};

use crate::health_incident::IncidentStatus;

/// An event of `IncidentMachine`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum IncidentEvent {
    /// An operator acknowledged the incident.
    Acknowledge,
    /// The cause was resolved.
    Resolve,
    /// The incident was closed.
    Close,
}

impl MachineState for IncidentStatus {
    fn stored(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Acknowledged => "acknowledged",
            Self::Resolved => "resolved",
            Self::Closed => "closed",
        }
    }
}

impl MachineEvent for IncidentEvent {
    fn name(self) -> &'static str {
        match self {
            Self::Acknowledge => "acknowledge",
            Self::Resolve => "resolve",
            Self::Close => "close",
        }
    }
}

/// `IncidentMachine`: the closed edge table. Every other pair of state and event is refused.
pub static INCIDENT_MACHINE: MachineDef<IncidentStatus, IncidentEvent, &'static str> = MachineDef {
    name: "IncidentMachine",
    states: &[
        IncidentStatus::Open,
        IncidentStatus::Acknowledged,
        IncidentStatus::Resolved,
        IncidentStatus::Closed,
    ],
    initial: IncidentStatus::Open,
    terminals: &[IncidentStatus::Closed],
    events: &[
        IncidentEvent::Acknowledge,
        IncidentEvent::Resolve,
        IncidentEvent::Close,
    ],
    guards: &[],
    edges: &[
        Edge {
            id: "H01",
            from: IncidentStatus::Open,
            event: IncidentEvent::Acknowledge,
            guard: None,
            to: IncidentStatus::Acknowledged,
        },
        Edge {
            id: "H02",
            from: IncidentStatus::Acknowledged,
            event: IncidentEvent::Resolve,
            guard: None,
            to: IncidentStatus::Resolved,
        },
        Edge {
            id: "H03",
            from: IncidentStatus::Resolved,
            event: IncidentEvent::Close,
            guard: None,
            to: IncidentStatus::Closed,
        },
    ],
};

impl IncidentStatus {
    /// The one transition function of `IncidentMachine`.
    pub fn apply(self, event: IncidentEvent) -> Result<Self, TransitionRefused> {
        transition(&INCIDENT_MACHINE, self, event, |_| false).map(|edge| edge.to)
    }

    /// The event that requests status `to`. A requested `Open` has no event: nothing moves
    /// back to the initial state.
    pub fn event_to(to: Self) -> Option<IncidentEvent> {
        match to {
            Self::Open => None,
            Self::Acknowledged => Some(IncidentEvent::Acknowledge),
            Self::Resolved => Some(IncidentEvent::Resolve),
            Self::Closed => Some(IncidentEvent::Close),
        }
    }
}

impl StateMachineApply for IncidentStatus {
    type Event = IncidentEvent;
    type Facts = ();
    fn apply(&self, event: IncidentEvent, _facts: &()) -> Result<Self, TransitionRefused> {
        IncidentStatus::apply(*self, event)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_types::transition_refused::TransitionRefusalReason;

    // spec: MEDULLA-ORCH-VOCAB-R815.5
    #[test]
    fn every_legal_transition_applies() {
        let machine = &INCIDENT_MACHINE;
        assert_eq!(machine.edges.len(), 3);
        for edge in machine.edges {
            assert_eq!(edge.from.apply(edge.event), Ok(edge.to), "{}", edge.id);
        }
    }

    // spec: MEDULLA-ORCH-VOCAB-R815.5
    #[test]
    fn every_illegal_pair_is_refused_with_transition_refused() {
        let machine = &INCIDENT_MACHINE;
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
                assert_eq!(refused.reason, reason, "{state:?} {event:?}");
                assert_eq!(refused.machine, "IncidentMachine");
                assert_eq!(refused.from, state.stored());
                assert_eq!(refused.event, event.name());
                assert_eq!(refused.code().as_str(), "TRANSITION_REFUSED");
            }
        }
        assert_eq!(illegal, 4 * 3 - 3);
    }

    /// Lines that assign the status field outside `apply`.
    // spec: MEDULLA-ORCH-VOCAB-R815.5
    #[test]
    fn no_writer_sets_status_outside_apply() {
        let source = include_str!("health_incident.rs");
        let production = source.split("#[cfg(test)]").next().unwrap();
        let writes: Vec<&str> = production
            .lines()
            .map(str::trim)
            .filter(|l| !l.starts_with("//") && l.contains(".status = "))
            .collect();
        assert_eq!(writes, vec!["self.status = self.status.apply(event)?;"]);
    }
}
