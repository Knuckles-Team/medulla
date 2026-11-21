//! MEDULLA-FEDERATED-QUERY-R738.1: the capability matrix entry status (planned until its backend lands, then shipped) as one explicit state machine
//! (organization constitution Article XXXI).
//!
//! The machine runs on the `Copy` discriminant [`CapabilityStatusKind`]; any data a record carries beside its
//! state stays on the record and is set at the transition. [`CapabilityStatusKind::apply`] is the one
//! transition function, and an illegal pair is the typed `TRANSITION_REFUSED`. The ontology
//! description is `CapabilityMachine` in `state-machines-v1.ttl`.

use serde::{Deserialize, Serialize};

use crate::transition_refused::{
    transition, Edge, MachineDef, MachineEvent, MachineState, TransitionRefused,
};

/// The closed state set of `CapabilityMachine`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityStatusKind {
    /// `planned`.
    Planned,
    /// `shipped`.
    Shipped,
}

/// The closed event set of `CapabilityMachine`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CapabilityEvent {
    /// The backend of the entry landed.
    Ship,
}

impl MachineState for CapabilityStatusKind {
    fn stored(self) -> &'static str {
        match self {
            Self::Planned => "planned",
            Self::Shipped => "shipped",
        }
    }
}

impl MachineEvent for CapabilityEvent {
    fn name(self) -> &'static str {
        match self {
            Self::Ship => "Ship",
        }
    }
}

/// `CapabilityMachine`: the closed edge table. Every other pair is refused.
pub static CAPABILITY_MACHINE: MachineDef<CapabilityStatusKind, CapabilityEvent, &'static str> = MachineDef {
    name: "CapabilityMachine",
    states: &[CapabilityStatusKind::Planned, CapabilityStatusKind::Shipped],
    initial: CapabilityStatusKind::Planned,
    terminals: &[CapabilityStatusKind::Shipped],
    events: &[CapabilityEvent::Ship],
    guards: &[],
    edges: &[
        Edge {
            id: "CS01",
            from: CapabilityStatusKind::Planned,
            event: CapabilityEvent::Ship,
            guard: None,
            to: CapabilityStatusKind::Shipped,
        },
    ],
};

impl CapabilityStatusKind {
    /// Every state, in declaration order.
    pub const ALL: [CapabilityStatusKind; 2] = [CapabilityStatusKind::Planned, CapabilityStatusKind::Shipped];

    /// The state a new record takes.
    pub const fn initial() -> Self {
        CapabilityStatusKind::Planned
    }

    /// The one transition function of `CapabilityMachine`.
    pub fn apply(self, event: CapabilityEvent) -> Result<Self, TransitionRefused> {
        transition(&CAPABILITY_MACHINE, self, event, |_| false).map(|edge| edge.to)
    }

    /// The event of the edge `self -> to`, if the table has one.
    pub fn event_to(self, to: Self) -> Option<CapabilityEvent> {
        CAPABILITY_MACHINE
            .edges
            .iter()
            .find(|edge| edge.from == self && edge.to == to)
            .map(|edge| edge.event)
    }

    /// Move to `to` along its edge, or refuse with `TRANSITION_REFUSED`.
    pub fn move_to(self, to: Self) -> Result<Self, TransitionRefused> {
        match self.event_to(to) {
            Some(event) => self.apply(event),
            None => Err(TransitionRefused {
                machine: CAPABILITY_MACHINE.name.to_string(),
                from: self.stored().to_string(),
                event: format!("to:{}", to.stored()),
                reason: crate::transition_refused::TransitionRefusalReason::IllegalTransition,
            }),
        }
    }

    /// The state whose stored text is `text`.
    pub fn from_stored(text: &str) -> Option<Self> {
        CAPABILITY_MACHINE.state_from_stored(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transition_refused::TransitionRefusalReason;

    // spec: MEDULLA-FEDERATED-QUERY-R738.1
    #[test]
    fn every_legal_transition_applies() {
        assert_eq!(CAPABILITY_MACHINE.edges.len(), 1);
        for edge in CAPABILITY_MACHINE.edges {
            assert_eq!(edge.from.apply(edge.event), Ok(edge.to), "{}", edge.id);
            assert_eq!(edge.from.event_to(edge.to), Some(edge.event));
        }
        assert_eq!(CapabilityStatusKind::initial(), CAPABILITY_MACHINE.initial);
    }

    // spec: MEDULLA-FEDERATED-QUERY-R738.1
    #[test]
    fn every_illegal_pair_is_refused_with_transition_refused() {
        let machine = &CAPABILITY_MACHINE;
        for &state in machine.states {
            for &event in machine.events {
                if machine.edges.iter().any(|e| e.from == state && e.event == event) {
                    continue;
                }
                let refused = state.apply(event).unwrap_err();
                assert_eq!(refused.machine, "CapabilityMachine");
                assert_eq!(refused.from, state.stored());
                assert_eq!(refused.event, event.name());
                let expected = if machine.terminals.contains(&state) {
                    TransitionRefusalReason::TerminalState
                } else {
                    TransitionRefusalReason::IllegalTransition
                };
                assert_eq!(refused.reason, expected);
            }
        }
        for state in CapabilityStatusKind::ALL {
            assert_eq!(CapabilityStatusKind::from_stored(state.stored()), Some(state));
        }
        assert_eq!(CapabilityStatusKind::from_stored("weather"), None);
    }

    // spec: MEDULLA-FEDERATED-QUERY-R738.1
    #[test]
    fn no_writer_sets_status_outside_apply() {
        let source = include_str!("../../medulla-h-compute/src/viz.rs");
        assert!(source.contains("CapabilityStatusKind::from_stored"), "the writer routes through the machine");
        assert!(!source.contains("status: \""), "a writer sets the status outside apply: status: \"");
    }
}
