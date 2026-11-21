//! MEDULLA-DATAPLAT-VOCAB-R009.3: the status of one durable run unit as one explicit state machine
//! (organization constitution Article XXXI).
//!
//! `InFlight` is the initial state; the closed event set is `Complete`, `Fail`, `Abandon`. [`DurableUnitStatus::apply`] is the only function that
//! produces a new state, and a refused pair is the typed `TRANSITION_REFUSED` naming the machine,
//! the current state and the event. The ontology description is `DurableUnitMachine` in `state-machines-v1.ttl`.

use crate::durable_unit::DurableUnitStatus;
use crate::transition_refused::{
    transition, Edge, MachineDef, MachineEvent, MachineState, TransitionRefused,
};

/// The closed event set of `DurableUnitMachine`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DurableUnitEvent {
    /// The unit finishes.
    Complete,
    /// The unit fails.
    Fail,
    /// The unit is given up.
    Abandon,
}

impl MachineState for DurableUnitStatus {
    fn stored(self) -> &'static str {
        match self {
            Self::InFlight => "in_flight",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Abandoned => "abandoned",
        }
    }
}

impl MachineEvent for DurableUnitEvent {
    fn name(self) -> &'static str {
        match self {
            Self::Complete => "Complete",
            Self::Fail => "Fail",
            Self::Abandon => "Abandon",
        }
    }
}

/// `DurableUnitMachine`: the closed edge table. Every other pair is refused.
pub static DURABLE_UNIT_MACHINE: MachineDef<DurableUnitStatus, DurableUnitEvent, &'static str> =
    MachineDef {
        name: "DurableUnitMachine",
        states: &[
            DurableUnitStatus::InFlight,
            DurableUnitStatus::Completed,
            DurableUnitStatus::Failed,
            DurableUnitStatus::Abandoned,
        ],
        initial: DurableUnitStatus::InFlight,
        terminals: &[
            DurableUnitStatus::Completed,
            DurableUnitStatus::Failed,
            DurableUnitStatus::Abandoned,
        ],
        events: &[
            DurableUnitEvent::Complete,
            DurableUnitEvent::Fail,
            DurableUnitEvent::Abandon,
        ],
        guards: &[],
        edges: &[
            Edge {
                id: "DU01",
                from: DurableUnitStatus::InFlight,
                event: DurableUnitEvent::Complete,
                guard: None,
                to: DurableUnitStatus::Completed,
            },
            Edge {
                id: "DU02",
                from: DurableUnitStatus::InFlight,
                event: DurableUnitEvent::Fail,
                guard: None,
                to: DurableUnitStatus::Failed,
            },
            Edge {
                id: "DU03",
                from: DurableUnitStatus::InFlight,
                event: DurableUnitEvent::Abandon,
                guard: None,
                to: DurableUnitStatus::Abandoned,
            },
        ],
    };

impl DurableUnitStatus {
    /// The one transition function of `DurableUnitMachine`: the next state, or the typed refusal.
    pub fn apply(self, event: DurableUnitEvent) -> Result<Self, TransitionRefused> {
        transition(&DURABLE_UNIT_MACHINE, self, event, |_| false).map(|edge| edge.to)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transition_refused::TransitionRefusalReason;

    const MACHINES: &str = include_str!("../../medulla-core/ontology/state-machines-v1.ttl");

    // spec: MEDULLA-DATAPLAT-VOCAB-R009.3
    #[test]
    fn every_legal_transition_applies() {
        assert_eq!(DURABLE_UNIT_MACHINE.edges.len(), 3);
        for edge in DURABLE_UNIT_MACHINE.edges {
            assert_eq!(edge.from.apply(edge.event), Ok(edge.to), "{}", edge.id);
        }
        assert_eq!(
            DurableUnitStatus::InFlight.apply(DurableUnitEvent::Complete),
            Ok(DurableUnitStatus::Completed)
        );
        assert_eq!(
            DurableUnitStatus::InFlight.apply(DurableUnitEvent::Fail),
            Ok(DurableUnitStatus::Failed)
        );
        assert_eq!(
            DurableUnitStatus::InFlight.apply(DurableUnitEvent::Abandon),
            Ok(DurableUnitStatus::Abandoned)
        );
    }

    // spec: MEDULLA-DATAPLAT-VOCAB-R009.3
    #[test]
    fn every_illegal_pair_is_refused_with_transition_refused() {
        let machine = &DURABLE_UNIT_MACHINE;
        for &state in machine.states {
            for &event in machine.events {
                if machine
                    .edges
                    .iter()
                    .any(|e| e.from == state && e.event == event)
                {
                    continue;
                }
                let refused = state.apply(event).unwrap_err();
                assert_eq!(refused.machine, "DurableUnitMachine");
                assert_eq!(refused.from, state.stored());
                assert_eq!(refused.event, event.name());
                let expected = if machine.is_terminal(state) {
                    TransitionRefusalReason::TerminalState
                } else {
                    TransitionRefusalReason::IllegalTransition
                };
                assert_eq!(refused.reason, expected);
                assert_eq!(refused.code().as_str(), "TRANSITION_REFUSED");
            }
        }
    }

    // spec: MEDULLA-DATAPLAT-VOCAB-R009.3
    #[test]
    fn the_ontology_describes_the_same_machine() {
        assert!(MACHINES.contains(":DurableUnitMachine a :StateMachine"));
        assert!(MACHINES.contains(":DurableUnitMachine_InFlight a"));
        assert!(MACHINES.contains(":storedForm \"in_flight\""));
        assert!(MACHINES.contains(":DurableUnitMachine_Completed a"));
        assert!(MACHINES.contains(":storedForm \"completed\""));
        assert!(MACHINES.contains(":DurableUnitMachine_Failed a"));
        assert!(MACHINES.contains(":storedForm \"failed\""));
        assert!(MACHINES.contains(":DurableUnitMachine_Abandoned a"));
        assert!(MACHINES.contains(":storedForm \"abandoned\""));
    }

    /// Lines that assign the `status` field outside the transition function.
    // spec: MEDULLA-DATAPLAT-VOCAB-R009.3
    #[test]
    fn no_writer_sets_status_outside_apply() {
        let sources = [include_str!("durable_unit.rs")];
        for source in sources {
            let production = source.split("#[cfg(test)]").next().unwrap();
            let writes: Vec<&str> = production
                .lines()
                .map(str::trim)
                .filter(|l| !l.starts_with("//") && l.contains(".status = "))
                .collect();
            assert!(writes.is_empty(), "{writes:?}");
        }
    }
}
