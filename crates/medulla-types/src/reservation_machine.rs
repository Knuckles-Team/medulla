//! MEDULLA-DECISION-ENGINE-R036.103 and R036.104: the reservation lifecycle as one explicit
//! state machine, shared by the RBAC read reservation and the Agent Library admission.
//!
//! `Reserved` is the initial state. The coordinator decision is the closed event set:
//! `Commit` enters `Committed`, `Abort` enters `Aborted`; both are terminal.
//! [`ReservationState::apply`] is the only function that produces a new state, and a refused
//! event is the typed `TRANSITION_REFUSED` naming the machine, the current state and the event.
//! The ontology description is `ReservationMachine` in `state-machines-v1.ttl`.

use serde::{Deserialize, Serialize};

use crate::transition_refused::{
    transition, Edge, MachineDef, MachineEvent, MachineState, TransitionRefused,
};

/// The state of one reservation: the closed state set of `ReservationMachine`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReservationState {
    /// The reservation is held and fences conflicting writes.
    Reserved,
    /// The coordinator decided COMMIT; the fence is released.
    Committed,
    /// The coordinator decided ABORT; the fence is released.
    Aborted,
}

/// The closed event set of `ReservationMachine`: the durable coordinator decision.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ReservationEvent {
    /// The coordinator decided COMMIT.
    Commit,
    /// The coordinator decided ABORT.
    Abort,
}

impl MachineState for ReservationState {
    fn stored(self) -> &'static str {
        match self {
            Self::Reserved => "reserved",
            Self::Committed => "committed",
            Self::Aborted => "aborted",
        }
    }
}

impl MachineEvent for ReservationEvent {
    fn name(self) -> &'static str {
        match self {
            Self::Commit => "Commit",
            Self::Abort => "Abort",
        }
    }
}

impl ReservationEvent {
    /// The state this event enters.
    pub fn target(self) -> ReservationState {
        match self {
            Self::Commit => ReservationState::Committed,
            Self::Abort => ReservationState::Aborted,
        }
    }
}

/// `ReservationMachine`: the closed edge table. Every other pair is refused.
pub static RESERVATION_MACHINE: MachineDef<ReservationState, ReservationEvent, &'static str> =
    MachineDef {
        name: "ReservationMachine",
        states: &[
            ReservationState::Reserved,
            ReservationState::Committed,
            ReservationState::Aborted,
        ],
        initial: ReservationState::Reserved,
        terminals: &[ReservationState::Committed, ReservationState::Aborted],
        events: &[ReservationEvent::Commit, ReservationEvent::Abort],
        guards: &[],
        edges: &[
            Edge {
                id: "R01",
                from: ReservationState::Reserved,
                event: ReservationEvent::Commit,
                guard: None,
                to: ReservationState::Committed,
            },
            Edge {
                id: "R02",
                from: ReservationState::Reserved,
                event: ReservationEvent::Abort,
                guard: None,
                to: ReservationState::Aborted,
            },
        ],
    };

impl ReservationState {
    /// The one transition function of `ReservationMachine`.
    pub fn apply(self, event: ReservationEvent) -> Result<Self, TransitionRefused> {
        transition(&RESERVATION_MACHINE, self, event, |_| false).map(|edge| edge.to)
    }

    /// Apply a decision that may repeat: the decision already applied replays as the
    /// current state; any other event goes through [`Self::apply`].
    pub fn resolve(self, event: ReservationEvent) -> Result<Self, TransitionRefused> {
        if self == event.target() {
            return Ok(self);
        }
        self.apply(event)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transition_refused::TransitionRefusalReason;

    const STATES: [ReservationState; 3] = [
        ReservationState::Reserved,
        ReservationState::Committed,
        ReservationState::Aborted,
    ];
    const EVENTS: [ReservationEvent; 2] = [ReservationEvent::Commit, ReservationEvent::Abort];

    #[test]
    fn every_legal_transition_is_taken_and_every_other_pair_refused() {
        for state in STATES {
            for event in EVENTS {
                let result = state.apply(event);
                let legal = state == ReservationState::Reserved;
                if legal {
                    assert_eq!(result, Ok(event.target()));
                } else {
                    let refused = result.unwrap_err();
                    assert_eq!(refused.machine, "ReservationMachine");
                    assert_eq!(refused.from, state.stored());
                    assert_eq!(refused.event, event.name());
                    assert_eq!(refused.reason, TransitionRefusalReason::TerminalState);
                }
            }
        }
    }

    #[test]
    fn a_repeated_decision_replays_and_the_opposite_one_is_refused() {
        assert_eq!(
            ReservationState::Committed.resolve(ReservationEvent::Commit),
            Ok(ReservationState::Committed)
        );
        assert_eq!(
            ReservationState::Aborted.resolve(ReservationEvent::Abort),
            Ok(ReservationState::Aborted)
        );
        assert!(ReservationState::Committed
            .resolve(ReservationEvent::Abort)
            .is_err());
        assert!(ReservationState::Aborted
            .resolve(ReservationEvent::Commit)
            .is_err());
    }

    const MACHINES: &str = include_str!("../../medulla-core/ontology/state-machines-v1.ttl");

    #[test]
    fn the_ontology_describes_the_same_machine() {
        assert!(MACHINES.contains(":ReservationMachine a :StateMachine"));
        for state in STATES {
            let name = format!(
                "{}{}",
                &state.stored()[..1].to_uppercase(),
                &state.stored()[1..]
            );
            assert!(
                MACHINES.contains(&format!(":ReservationMachine_{name} a")),
                "{name}"
            );
            assert!(MACHINES.contains(&format!(":storedForm \"{}\"", state.stored())));
        }
        for event in EVENTS {
            assert!(MACHINES.contains(&format!(":ReservationMachine_Event_{} a", event.name())));
        }
        for edge in RESERVATION_MACHINE.edges {
            let block = format!(":ReservationMachine_{} a :ReservationTransition", edge.id);
            let start = MACHINES.find(&block).expect("edge declared");
            let text = &MACHINES[start..];
            let text = &text[..text.find(" .\n").unwrap()];
            assert!(
                text.contains(&format!(":fromState :ReservationMachine_{}", {
                    let s = edge.from.stored();
                    format!("{}{}", s[..1].to_uppercase(), &s[1..])
                }))
            );
            assert!(text.contains(&format!(
                ":firedBy :ReservationMachine_Event_{}",
                edge.event.name()
            )));
        }
    }
}
