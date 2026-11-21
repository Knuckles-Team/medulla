//! MEDULLA-FEDERATED-QUERY-R738.7: the status of a failure-pattern gap topic as one explicit state machine
//! (organization constitution Article XXXI).
//!
//! The machine runs on the `Copy` discriminant [`GapStatusKind`]; any data a record carries beside its
//! state stays on the record and is set at the transition. [`GapStatusKind::apply`] is the one
//! transition function, and an illegal pair is the typed `TRANSITION_REFUSED`. The ontology
//! description is `GapTopicMachine` in `state-machines-v1.ttl`.

use serde::{Deserialize, Serialize};

use crate::transition_refused::{
    transition, Edge, MachineDef, MachineEvent, MachineState, TransitionRefused,
};

/// The closed state set of `GapTopicMachine`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GapStatusKind {
    /// `open`.
    Open,
    /// `addressed`.
    Addressed,
}

/// The closed event set of `GapTopicMachine`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GapTopicEvent {
    /// A source node covers the gap.
    Address,
}

impl MachineState for GapStatusKind {
    fn stored(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Addressed => "addressed",
        }
    }
}

impl MachineEvent for GapTopicEvent {
    fn name(self) -> &'static str {
        match self {
            Self::Address => "Address",
        }
    }
}

/// `GapTopicMachine`: the closed edge table. Every other pair is refused.
pub static GAP_TOPIC_MACHINE: MachineDef<GapStatusKind, GapTopicEvent, &'static str> = MachineDef {
    name: "GapTopicMachine",
    states: &[GapStatusKind::Open, GapStatusKind::Addressed],
    initial: GapStatusKind::Open,
    terminals: &[GapStatusKind::Addressed],
    events: &[GapTopicEvent::Address],
    guards: &[],
    edges: &[
        Edge {
            id: "GT01",
            from: GapStatusKind::Open,
            event: GapTopicEvent::Address,
            guard: None,
            to: GapStatusKind::Addressed,
        },
    ],
};

impl GapStatusKind {
    /// Every state, in declaration order.
    pub const ALL: [GapStatusKind; 2] = [GapStatusKind::Open, GapStatusKind::Addressed];

    /// The state a new record takes.
    pub const fn initial() -> Self {
        GapStatusKind::Open
    }

    /// The one transition function of `GapTopicMachine`.
    pub fn apply(self, event: GapTopicEvent) -> Result<Self, TransitionRefused> {
        transition(&GAP_TOPIC_MACHINE, self, event, |_| false).map(|edge| edge.to)
    }

    /// The event of the edge `self -> to`, if the table has one.
    pub fn event_to(self, to: Self) -> Option<GapTopicEvent> {
        GAP_TOPIC_MACHINE
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
                machine: GAP_TOPIC_MACHINE.name.to_string(),
                from: self.stored().to_string(),
                event: format!("to:{}", to.stored()),
                reason: crate::transition_refused::TransitionRefusalReason::IllegalTransition,
            }),
        }
    }

    /// The state whose stored text is `text`.
    pub fn from_stored(text: &str) -> Option<Self> {
        GAP_TOPIC_MACHINE.state_from_stored(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transition_refused::TransitionRefusalReason;

    // spec: MEDULLA-FEDERATED-QUERY-R738.7
    #[test]
    fn every_legal_transition_applies() {
        assert_eq!(GAP_TOPIC_MACHINE.edges.len(), 1);
        for edge in GAP_TOPIC_MACHINE.edges {
            assert_eq!(edge.from.apply(edge.event), Ok(edge.to), "{}", edge.id);
            assert_eq!(edge.from.event_to(edge.to), Some(edge.event));
        }
        assert_eq!(GapStatusKind::initial(), GAP_TOPIC_MACHINE.initial);
    }

    // spec: MEDULLA-FEDERATED-QUERY-R738.7
    #[test]
    fn every_illegal_pair_is_refused_with_transition_refused() {
        let machine = &GAP_TOPIC_MACHINE;
        for &state in machine.states {
            for &event in machine.events {
                if machine.edges.iter().any(|e| e.from == state && e.event == event) {
                    continue;
                }
                let refused = state.apply(event).unwrap_err();
                assert_eq!(refused.machine, "GapTopicMachine");
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
        for state in GapStatusKind::ALL {
            assert_eq!(GapStatusKind::from_stored(state.stored()), Some(state));
        }
        assert_eq!(GapStatusKind::from_stored("weather"), None);
    }

    // spec: MEDULLA-FEDERATED-QUERY-R738.7
    #[test]
    fn no_writer_sets_status_outside_apply() {
        let source = include_str!("../../medulla-decision-types/src/failure_patterns.rs");
        assert!(source.contains("GapStatus::initial"), "the writer routes through the machine");
        assert!(!source.contains("status: GapStatus::Open"), "a writer sets the status outside apply: status: GapStatus::Open");
    }
}
