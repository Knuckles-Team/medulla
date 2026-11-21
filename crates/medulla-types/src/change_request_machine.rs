//! MEDULLA-DATAPLAT-VOCAB-R009.2: the lifecycle of one stewardship change request as one explicit state machine
//! (organization constitution Article XXXI).
//!
//! `Open` is the initial state; the closed event set is `Approve`, `Reject`, `Apply`. [`ChangeRequestState::apply`] is the only function that
//! produces a new state, and a refused pair is the typed `TRANSITION_REFUSED` naming the machine,
//! the current state and the event. The ontology description is `ChangeRequestMachine` in `state-machines-v1.ttl`.

use serde::{Deserialize, Serialize};

use crate::transition_refused::{
    transition, Edge, MachineDef, MachineEvent, MachineState, TransitionRefused,
};

/// Where a change request is in its lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum ChangeRequestState {
    /// Awaiting review; the initial state.
    Open,
    /// Approved by a reviewer, not yet applied.
    Approved,
    /// Refused; terminal.
    Rejected,
    /// The proposed change was applied; terminal.
    Applied,
}

/// The closed event set of `ChangeRequestMachine`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ChangeRequestEvent {
    /// A reviewer approves the request.
    Approve,
    /// A reviewer rejects the request.
    Reject,
    /// The approved change is applied.
    Apply,
}

impl MachineState for ChangeRequestState {
    fn stored(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Approved => "approved",
            Self::Rejected => "rejected",
            Self::Applied => "applied",
        }
    }
}

impl MachineEvent for ChangeRequestEvent {
    fn name(self) -> &'static str {
        match self {
            Self::Approve => "Approve",
            Self::Reject => "Reject",
            Self::Apply => "Apply",
        }
    }
}

/// `ChangeRequestMachine`: the closed edge table. Every other pair is refused.
pub static CHANGE_REQUEST_MACHINE: MachineDef<
    ChangeRequestState,
    ChangeRequestEvent,
    &'static str,
> = MachineDef {
    name: "ChangeRequestMachine",
    states: &[
        ChangeRequestState::Open,
        ChangeRequestState::Approved,
        ChangeRequestState::Rejected,
        ChangeRequestState::Applied,
    ],
    initial: ChangeRequestState::Open,
    terminals: &[ChangeRequestState::Rejected, ChangeRequestState::Applied],
    events: &[
        ChangeRequestEvent::Approve,
        ChangeRequestEvent::Reject,
        ChangeRequestEvent::Apply,
    ],
    guards: &[],
    edges: &[
        Edge {
            id: "CR01",
            from: ChangeRequestState::Open,
            event: ChangeRequestEvent::Approve,
            guard: None,
            to: ChangeRequestState::Approved,
        },
        Edge {
            id: "CR02",
            from: ChangeRequestState::Open,
            event: ChangeRequestEvent::Reject,
            guard: None,
            to: ChangeRequestState::Rejected,
        },
        Edge {
            id: "CR03",
            from: ChangeRequestState::Approved,
            event: ChangeRequestEvent::Apply,
            guard: None,
            to: ChangeRequestState::Applied,
        },
    ],
};

impl ChangeRequestState {
    /// The one transition function of `ChangeRequestMachine`: the next state, or the typed refusal.
    pub fn apply(self, event: ChangeRequestEvent) -> Result<Self, TransitionRefused> {
        transition(&CHANGE_REQUEST_MACHINE, self, event, |_| false).map(|edge| edge.to)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transition_refused::TransitionRefusalReason;

    const MACHINES: &str = include_str!("../../medulla-core/ontology/state-machines-v1.ttl");

    // spec: MEDULLA-DATAPLAT-VOCAB-R009.2
    #[test]
    fn every_legal_transition_applies() {
        assert_eq!(CHANGE_REQUEST_MACHINE.edges.len(), 3);
        for edge in CHANGE_REQUEST_MACHINE.edges {
            assert_eq!(edge.from.apply(edge.event), Ok(edge.to), "{}", edge.id);
        }
        assert_eq!(
            ChangeRequestState::Open.apply(ChangeRequestEvent::Approve),
            Ok(ChangeRequestState::Approved)
        );
        assert_eq!(
            ChangeRequestState::Open.apply(ChangeRequestEvent::Reject),
            Ok(ChangeRequestState::Rejected)
        );
        assert_eq!(
            ChangeRequestState::Approved.apply(ChangeRequestEvent::Apply),
            Ok(ChangeRequestState::Applied)
        );
    }

    // spec: MEDULLA-DATAPLAT-VOCAB-R009.2
    #[test]
    fn every_illegal_pair_is_refused_with_transition_refused() {
        let machine = &CHANGE_REQUEST_MACHINE;
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
                assert_eq!(refused.machine, "ChangeRequestMachine");
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

    // spec: MEDULLA-DATAPLAT-VOCAB-R009.2
    #[test]
    fn the_ontology_describes_the_same_machine() {
        assert!(MACHINES.contains(":ChangeRequestMachine a :StateMachine"));
        assert!(MACHINES.contains(":ChangeRequestMachine_Open a"));
        assert!(MACHINES.contains(":storedForm \"open\""));
        assert!(MACHINES.contains(":ChangeRequestMachine_Approved a"));
        assert!(MACHINES.contains(":storedForm \"approved\""));
        assert!(MACHINES.contains(":ChangeRequestMachine_Rejected a"));
        assert!(MACHINES.contains(":storedForm \"rejected\""));
        assert!(MACHINES.contains(":ChangeRequestMachine_Applied a"));
        assert!(MACHINES.contains(":storedForm \"applied\""));
    }

    /// Lines that assign the `state` field outside the transition function.
    // spec: MEDULLA-DATAPLAT-VOCAB-R009.2
    #[test]
    fn no_writer_sets_state_outside_apply() {
        let sources = [
            include_str!("../../medulla-catalog-types/src/stewardship.rs"),
            include_str!("../../medulla-core/src/stewardship.rs"),
        ];
        for source in sources {
            let production = source.split("#[cfg(test)]").next().unwrap();
            let writes: Vec<&str> = production
                .lines()
                .map(str::trim)
                .filter(|l| !l.starts_with("//") && l.contains(".state = "))
                .collect();
            assert!(writes.is_empty(), "{writes:?}");
        }
    }
}
