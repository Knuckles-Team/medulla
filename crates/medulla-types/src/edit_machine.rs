//! MEDULLA-DECISION-ENGINE-R173.7 -- the lifecycle of a harness edit as one explicit state
//! machine (organization constitution Article XXXI).
//!
//! A harness edit is proposed, shipped and possibly reverted. The engine owns the
//! `core:editStatus` literal of its node: [`EditStatus::apply`] is the one transition
//! function and returns the typed [`TransitionRefused`] for every pair the edge table does
//! not list. The enum lives in this leaf types crate so the concentration question
//! (`medulla-decision-outcome-types`) reads the same closed status.

use serde::{Deserialize, Serialize};

use crate::transition_refused::{
    transition, Edge, MachineDef, MachineEvent, MachineState, TransitionRefused,
};

/// The recorded status of a harness edit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum EditStatus {
    /// Proposed, not yet shipped.
    Proposed,
    /// Shipped: the concentration cap counts it.
    Shipped,
    /// Reverted; never moves again.
    Reverted,
}

impl MachineState for EditStatus {
    fn stored(self) -> &'static str {
        match self {
            Self::Proposed => "proposed",
            Self::Shipped => "shipped",
            Self::Reverted => "reverted",
        }
    }
}

/// The closed event set of the edit machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum EditEvent {
    /// The edit ships.
    Ship,
    /// The edit is reverted.
    Revert,
}

impl MachineEvent for EditEvent {
    fn name(self) -> &'static str {
        match self {
            Self::Ship => "Ship",
            Self::Revert => "Revert",
        }
    }
}

/// `EditMachine`: the closed edge table. Every other pair of state and event is refused.
pub const EDIT_MACHINE: MachineDef<EditStatus, EditEvent, &'static str> = MachineDef {
    name: "EditMachine",
    states: &[
        EditStatus::Proposed,
        EditStatus::Shipped,
        EditStatus::Reverted,
    ],
    initial: EditStatus::Proposed,
    terminals: &[EditStatus::Reverted],
    events: &[EditEvent::Ship, EditEvent::Revert],
    guards: &[],
    edges: &[
        Edge {
            id: "E01",
            from: EditStatus::Proposed,
            event: EditEvent::Ship,
            guard: None,
            to: EditStatus::Shipped,
        },
        Edge {
            id: "E02",
            from: EditStatus::Proposed,
            event: EditEvent::Revert,
            guard: None,
            to: EditStatus::Reverted,
        },
        Edge {
            id: "E03",
            from: EditStatus::Shipped,
            event: EditEvent::Revert,
            guard: None,
            to: EditStatus::Reverted,
        },
    ],
};

impl EditStatus {
    /// The status whose stored text is `stored`, if the machine declares one.
    pub fn from_stored(stored: &str) -> Option<Self> {
        EDIT_MACHINE.state_from_stored(stored)
    }

    /// The one transition function of `EditMachine`.
    pub fn apply(self, event: EditEvent) -> Result<Self, TransitionRefused> {
        transition(&EDIT_MACHINE, self, event, |_| false).map(|edge| edge.to)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transition_refused::TransitionRefusalReason;

    // spec: MEDULLA-DECISION-ENGINE-R173.7
    #[test]
    fn every_legal_transition_applies() {
        for edge in EDIT_MACHINE.edges {
            assert_eq!(edge.from.apply(edge.event), Ok(edge.to), "{}", edge.id);
        }
        for status in EDIT_MACHINE.states {
            assert_eq!(EditStatus::from_stored(status.stored()), Some(*status));
        }
        assert_eq!(EditStatus::from_stored("rejected"), None);
    }

    // spec: MEDULLA-DECISION-ENGINE-R173.7
    #[test]
    fn every_illegal_pair_is_refused_with_transition_refused() {
        let machine = &EDIT_MACHINE;
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
                let expected = if machine.is_terminal(state) {
                    TransitionRefusalReason::TerminalState
                } else {
                    TransitionRefusalReason::IllegalTransition
                };
                assert_eq!(refused.reason, expected);
                assert_eq!(refused.machine, "EditMachine");
                assert_eq!(refused.from, state.stored());
                assert_eq!(refused.event, event.name());
                assert_eq!(refused.code().as_str(), "TRANSITION_REFUSED");
            }
        }
        assert_eq!(
            illegal,
            machine.states.len() * machine.events.len() - machine.edges.len()
        );
    }

    /// No production line assigns an edit status from a literal variant.
    // spec: MEDULLA-DECISION-ENGINE-R173.7
    #[test]
    fn no_writer_sets_status_outside_apply() {
        for source in [
            include_str!("../../medulla-h-graph/src/graph_ops/harness_edit.rs"),
            include_str!("../../medulla-decision-outcome-types/src/decision/concentration.rs"),
        ] {
            let production = source.split("#[cfg(test)]").next().unwrap();
            let writes: Vec<&str> = production
                .lines()
                .map(str::trim)
                .filter(|l| !l.starts_with("//") && l.contains(" = EditStatus::") && !l.contains("=="))
                .collect();
            assert!(writes.is_empty(), "{writes:?}");
        }
    }
}
