//! MEDULLA-FEDERATED-QUERY-R738.3: the lifecycle of a research claim as one explicit state machine
//! (organization constitution Article XXXI).
//!
//! The machine runs on the `Copy` discriminant [`ClaimStateKind`]; any data a record carries beside its
//! state stays on the record and is set at the transition. [`ClaimStateKind::apply`] is the one
//! transition function, and an illegal pair is the typed `TRANSITION_REFUSED`. The ontology
//! description is `ClaimMachine` in `state-machines-v1.ttl`.

use serde::{Deserialize, Serialize};

use crate::transition_refused::{
    transition, Edge, MachineDef, MachineEvent, MachineState, TransitionRefused,
};

/// The closed state set of `ClaimMachine`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClaimStateKind {
    /// `proposed`.
    Proposed,
    /// `supported`.
    Supported,
    /// `contested`.
    Contested,
    /// `accepted`.
    Accepted,
    /// `retired`.
    Retired,
}

/// The closed event set of `ClaimMachine`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ClaimEvent {
    /// Evidence moved the claim to supported.
    Support,
    /// Evidence moved the claim to contested.
    Contest,
    /// Evidence moved the claim to accepted.
    Accept,
    /// The claim was retired.
    Retire,
}

impl MachineState for ClaimStateKind {
    fn stored(self) -> &'static str {
        match self {
            Self::Proposed => "proposed",
            Self::Supported => "supported",
            Self::Contested => "contested",
            Self::Accepted => "accepted",
            Self::Retired => "retired",
        }
    }
}

impl MachineEvent for ClaimEvent {
    fn name(self) -> &'static str {
        match self {
            Self::Support => "Support",
            Self::Contest => "Contest",
            Self::Accept => "Accept",
            Self::Retire => "Retire",
        }
    }
}

/// `ClaimMachine`: the closed edge table. Every other pair is refused.
pub static CLAIM_MACHINE: MachineDef<ClaimStateKind, ClaimEvent, &'static str> = MachineDef {
    name: "ClaimMachine",
    states: &[ClaimStateKind::Proposed, ClaimStateKind::Supported, ClaimStateKind::Contested, ClaimStateKind::Accepted, ClaimStateKind::Retired],
    initial: ClaimStateKind::Proposed,
    terminals: &[ClaimStateKind::Retired],
    events: &[ClaimEvent::Support, ClaimEvent::Contest, ClaimEvent::Accept, ClaimEvent::Retire],
    guards: &[],
    edges: &[
        Edge {
            id: "CL01",
            from: ClaimStateKind::Proposed,
            event: ClaimEvent::Support,
            guard: None,
            to: ClaimStateKind::Supported,
        },
        Edge {
            id: "CL02",
            from: ClaimStateKind::Proposed,
            event: ClaimEvent::Contest,
            guard: None,
            to: ClaimStateKind::Contested,
        },
        Edge {
            id: "CL03",
            from: ClaimStateKind::Proposed,
            event: ClaimEvent::Retire,
            guard: None,
            to: ClaimStateKind::Retired,
        },
        Edge {
            id: "CL04",
            from: ClaimStateKind::Supported,
            event: ClaimEvent::Accept,
            guard: None,
            to: ClaimStateKind::Accepted,
        },
        Edge {
            id: "CL05",
            from: ClaimStateKind::Supported,
            event: ClaimEvent::Contest,
            guard: None,
            to: ClaimStateKind::Contested,
        },
        Edge {
            id: "CL06",
            from: ClaimStateKind::Supported,
            event: ClaimEvent::Retire,
            guard: None,
            to: ClaimStateKind::Retired,
        },
        Edge {
            id: "CL07",
            from: ClaimStateKind::Contested,
            event: ClaimEvent::Support,
            guard: None,
            to: ClaimStateKind::Supported,
        },
        Edge {
            id: "CL08",
            from: ClaimStateKind::Contested,
            event: ClaimEvent::Retire,
            guard: None,
            to: ClaimStateKind::Retired,
        },
        Edge {
            id: "CL09",
            from: ClaimStateKind::Accepted,
            event: ClaimEvent::Contest,
            guard: None,
            to: ClaimStateKind::Contested,
        },
        Edge {
            id: "CL10",
            from: ClaimStateKind::Accepted,
            event: ClaimEvent::Retire,
            guard: None,
            to: ClaimStateKind::Retired,
        },
    ],
};

impl ClaimStateKind {
    /// Every state, in declaration order.
    pub const ALL: [ClaimStateKind; 5] = [ClaimStateKind::Proposed, ClaimStateKind::Supported, ClaimStateKind::Contested, ClaimStateKind::Accepted, ClaimStateKind::Retired];

    /// The state a new record takes.
    pub const fn initial() -> Self {
        ClaimStateKind::Proposed
    }

    /// The one transition function of `ClaimMachine`.
    pub fn apply(self, event: ClaimEvent) -> Result<Self, TransitionRefused> {
        transition(&CLAIM_MACHINE, self, event, |_| false).map(|edge| edge.to)
    }

    /// The event of the edge `self -> to`, if the table has one.
    pub fn event_to(self, to: Self) -> Option<ClaimEvent> {
        CLAIM_MACHINE
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
                machine: CLAIM_MACHINE.name.to_string(),
                from: self.stored().to_string(),
                event: format!("to:{}", to.stored()),
                reason: crate::transition_refused::TransitionRefusalReason::IllegalTransition,
            }),
        }
    }

    /// The state whose stored text is `text`.
    pub fn from_stored(text: &str) -> Option<Self> {
        CLAIM_MACHINE.state_from_stored(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transition_refused::TransitionRefusalReason;

    // proves R738.3: ClaimRecord/ClaimState is one explicit state machine, and every writer routes ...
    // spec: MEDULLA-FEDERATED-QUERY-R738.3
    #[test]
    fn every_legal_transition_applies() {
        assert_eq!(CLAIM_MACHINE.edges.len(), 10);
        for edge in CLAIM_MACHINE.edges {
            assert_eq!(edge.from.apply(edge.event), Ok(edge.to), "{}", edge.id);
            assert_eq!(edge.from.event_to(edge.to), Some(edge.event));
        }
        assert_eq!(ClaimStateKind::initial(), CLAIM_MACHINE.initial);
    }

    // spec: MEDULLA-FEDERATED-QUERY-R738.3
    #[test]
    fn every_illegal_pair_is_refused_with_transition_refused() {
        let machine = &CLAIM_MACHINE;
        for &state in machine.states {
            for &event in machine.events {
                if machine.edges.iter().any(|e| e.from == state && e.event == event) {
                    continue;
                }
                let refused = state.apply(event).unwrap_err();
                assert_eq!(refused.machine, "ClaimMachine");
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
        for state in ClaimStateKind::ALL {
            assert_eq!(ClaimStateKind::from_stored(state.stored()), Some(state));
        }
        assert_eq!(ClaimStateKind::from_stored("weather"), None);
    }

    // spec: MEDULLA-FEDERATED-QUERY-R738.3
    #[test]
    fn no_writer_sets_state_outside_apply() {
        let source = include_str!("../../medulla-epistemic/src/claims.rs");
        assert!(source.contains("legal_transition"), "the writer routes through the machine");
        assert!(!source.contains("LEGAL_TRANSITIONS"), "a writer sets the state outside apply: LEGAL_TRANSITIONS");
    }
}
