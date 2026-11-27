//! MEDULLA-FEDERATED-QUERY-R738.5: the status of a tracked contradiction: open, proposed, resolved in that order only as one explicit state machine
//! (organization constitution Article XXXI).
//!
//! The machine runs on the `Copy` discriminant [`ContradictionStatusKind`]; any data a record carries beside its
//! state stays on the record and is set at the transition. [`ContradictionStatusKind::apply`] is the one
//! transition function, and an illegal pair is the typed `TRANSITION_REFUSED`. The ontology
//! description is `ContradictionMachine` in `state-machines-v1.ttl`.

use serde::{Deserialize, Serialize};

use crate::transition_refused::{
    transition, Edge, MachineDef, MachineEvent, MachineState, TransitionRefused,
};

/// The closed state set of `ContradictionMachine`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContradictionStatusKind {
    /// `open`.
    Open,
    /// `proposed`.
    Proposed,
    /// `resolved`.
    Resolved,
}

/// The closed event set of `ContradictionMachine`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ContradictionEvent {
    /// A resolution was proposed.
    Propose,
    /// The proposed resolution was applied.
    Resolve,
}

impl MachineState for ContradictionStatusKind {
    fn stored(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Proposed => "proposed",
            Self::Resolved => "resolved",
        }
    }
}

impl MachineEvent for ContradictionEvent {
    fn name(self) -> &'static str {
        match self {
            Self::Propose => "Propose",
            Self::Resolve => "Resolve",
        }
    }
}

/// `ContradictionMachine`: the closed edge table. Every other pair is refused.
pub static CONTRADICTION_MACHINE: MachineDef<
    ContradictionStatusKind,
    ContradictionEvent,
    &'static str,
> = MachineDef {
    name: "ContradictionMachine",
    states: &[
        ContradictionStatusKind::Open,
        ContradictionStatusKind::Proposed,
        ContradictionStatusKind::Resolved,
    ],
    initial: ContradictionStatusKind::Open,
    terminals: &[ContradictionStatusKind::Resolved],
    events: &[ContradictionEvent::Propose, ContradictionEvent::Resolve],
    guards: &[],
    edges: &[
        Edge {
            id: "K01",
            from: ContradictionStatusKind::Open,
            event: ContradictionEvent::Propose,
            guard: None,
            to: ContradictionStatusKind::Proposed,
        },
        Edge {
            id: "K02",
            from: ContradictionStatusKind::Proposed,
            event: ContradictionEvent::Resolve,
            guard: None,
            to: ContradictionStatusKind::Resolved,
        },
    ],
};

impl ContradictionStatusKind {
    /// Every state, in declaration order.
    pub const ALL: [ContradictionStatusKind; 3] = [
        ContradictionStatusKind::Open,
        ContradictionStatusKind::Proposed,
        ContradictionStatusKind::Resolved,
    ];

    /// The state a new record takes.
    pub const fn initial() -> Self {
        ContradictionStatusKind::Open
    }

    /// The one transition function of `ContradictionMachine`.
    pub fn apply(self, event: ContradictionEvent) -> Result<Self, TransitionRefused> {
        transition(&CONTRADICTION_MACHINE, self, event, |_| false).map(|edge| edge.to)
    }

    /// The event of the edge `self -> to`, if the table has one.
    pub fn event_to(self, to: Self) -> Option<ContradictionEvent> {
        CONTRADICTION_MACHINE
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
                machine: CONTRADICTION_MACHINE.name.to_string(),
                from: self.stored().to_string(),
                event: format!("to:{}", to.stored()),
                reason: crate::transition_refused::TransitionRefusalReason::IllegalTransition,
            }),
        }
    }

    /// The state whose stored text is `text`.
    pub fn from_stored(text: &str) -> Option<Self> {
        CONTRADICTION_MACHINE.state_from_stored(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transition_refused::TransitionRefusalReason;

    // proves R738.5: ContradictionRecord/ContradictionStatus is one explicit state machine, and ...
    // spec: MEDULLA-FEDERATED-QUERY-R738.5
    #[test]
    fn every_legal_transition_applies() {
        assert_eq!(CONTRADICTION_MACHINE.edges.len(), 2);
        for edge in CONTRADICTION_MACHINE.edges {
            assert_eq!(edge.from.apply(edge.event), Ok(edge.to), "{}", edge.id);
            assert_eq!(edge.from.event_to(edge.to), Some(edge.event));
        }
        assert_eq!(
            ContradictionStatusKind::initial(),
            CONTRADICTION_MACHINE.initial
        );
    }

    // spec: MEDULLA-FEDERATED-QUERY-R738.5
    #[test]
    fn every_illegal_pair_is_refused_with_transition_refused() {
        let machine = &CONTRADICTION_MACHINE;
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
                assert_eq!(refused.machine, "ContradictionMachine");
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
        for state in ContradictionStatusKind::ALL {
            assert_eq!(
                ContradictionStatusKind::from_stored(state.stored()),
                Some(state)
            );
        }
        assert_eq!(ContradictionStatusKind::from_stored("weather"), None);
    }

    // spec: MEDULLA-FEDERATED-QUERY-R738.5
    #[test]
    fn no_writer_sets_status_outside_apply() {
        let source = include_str!("../../medulla-h-decide/src/decide/belief_revise.rs");
        assert!(
            source.contains("move_to"),
            "the writer routes through the machine"
        );
        assert!(
            !source.contains("record.status = to;"),
            "a writer sets the status outside apply: record.status = to;"
        );
    }
}
