//! MEDULLA-DATAPLAT-VOCAB-R009.1: the lifecycle of one data-product access request as one explicit state machine
//! (organization constitution Article XXXI).
//!
//! `Pending` is the initial state; the closed event set is `Approve`, `Reject`, `Revoke`. [`AccessRequestState::apply`] is the only function that
//! produces a new state, and a refused pair is the typed `TRANSITION_REFUSED` naming the machine,
//! the current state and the event. The ontology description is `AccessRequestMachine` in `state-machines-v1.ttl`.

use serde::{Deserialize, Serialize};

use crate::transition_refused::{
    transition, Edge, MachineDef, MachineEvent, MachineState, TransitionRefused,
};

/// Where an access request is in its lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum AccessRequestState {
    /// Awaiting a decision; the initial state.
    Pending,
    /// Approved; a sharing grant was written.
    Approved,
    /// Refused; terminal.
    Rejected,
    /// The approval was withdrawn; terminal.
    Revoked,
}

/// The closed event set of `AccessRequestMachine`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AccessRequestEvent {
    /// A reviewer approves the request.
    Approve,
    /// A reviewer rejects the request.
    Reject,
    /// The approval is withdrawn.
    Revoke,
}

impl MachineState for AccessRequestState {
    fn stored(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Approved => "approved",
            Self::Rejected => "rejected",
            Self::Revoked => "revoked",
        }
    }
}

impl MachineEvent for AccessRequestEvent {
    fn name(self) -> &'static str {
        match self {
            Self::Approve => "Approve",
            Self::Reject => "Reject",
            Self::Revoke => "Revoke",
        }
    }
}

/// `AccessRequestMachine`: the closed edge table. Every other pair is refused.
pub static ACCESS_REQUEST_MACHINE: MachineDef<
    AccessRequestState,
    AccessRequestEvent,
    &'static str,
> = MachineDef {
    name: "AccessRequestMachine",
    states: &[
        AccessRequestState::Pending,
        AccessRequestState::Approved,
        AccessRequestState::Rejected,
        AccessRequestState::Revoked,
    ],
    initial: AccessRequestState::Pending,
    terminals: &[AccessRequestState::Rejected, AccessRequestState::Revoked],
    events: &[
        AccessRequestEvent::Approve,
        AccessRequestEvent::Reject,
        AccessRequestEvent::Revoke,
    ],
    guards: &[],
    edges: &[
        Edge {
            id: "AR01",
            from: AccessRequestState::Pending,
            event: AccessRequestEvent::Approve,
            guard: None,
            to: AccessRequestState::Approved,
        },
        Edge {
            id: "AR02",
            from: AccessRequestState::Pending,
            event: AccessRequestEvent::Reject,
            guard: None,
            to: AccessRequestState::Rejected,
        },
        Edge {
            id: "AR03",
            from: AccessRequestState::Approved,
            event: AccessRequestEvent::Revoke,
            guard: None,
            to: AccessRequestState::Revoked,
        },
    ],
};

impl AccessRequestState {
    /// The one transition function of `AccessRequestMachine`: the next state, or the typed refusal.
    pub fn apply(self, event: AccessRequestEvent) -> Result<Self, TransitionRefused> {
        transition(&ACCESS_REQUEST_MACHINE, self, event, |_| false).map(|edge| edge.to)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transition_refused::TransitionRefusalReason;

    const MACHINES: &str = include_str!("../../medulla-core/ontology/state-machines-v1.ttl");

    // spec: MEDULLA-DATAPLAT-VOCAB-R009.1
    #[test]
    fn every_legal_transition_applies() {
        assert_eq!(ACCESS_REQUEST_MACHINE.edges.len(), 3);
        for edge in ACCESS_REQUEST_MACHINE.edges {
            assert_eq!(edge.from.apply(edge.event), Ok(edge.to), "{}", edge.id);
        }
        assert_eq!(
            AccessRequestState::Pending.apply(AccessRequestEvent::Approve),
            Ok(AccessRequestState::Approved)
        );
        assert_eq!(
            AccessRequestState::Pending.apply(AccessRequestEvent::Reject),
            Ok(AccessRequestState::Rejected)
        );
        assert_eq!(
            AccessRequestState::Approved.apply(AccessRequestEvent::Revoke),
            Ok(AccessRequestState::Revoked)
        );
    }

    // spec: MEDULLA-DATAPLAT-VOCAB-R009.1
    #[test]
    fn every_illegal_pair_is_refused_with_transition_refused() {
        let machine = &ACCESS_REQUEST_MACHINE;
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
                assert_eq!(refused.machine, "AccessRequestMachine");
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

    // spec: MEDULLA-DATAPLAT-VOCAB-R009.1
    #[test]
    fn the_ontology_describes_the_same_machine() {
        assert!(MACHINES.contains(":AccessRequestMachine a :StateMachine"));
        assert!(MACHINES.contains(":AccessRequestMachine_Pending a"));
        assert!(MACHINES.contains(":storedForm \"pending\""));
        assert!(MACHINES.contains(":AccessRequestMachine_Approved a"));
        assert!(MACHINES.contains(":storedForm \"approved\""));
        assert!(MACHINES.contains(":AccessRequestMachine_Rejected a"));
        assert!(MACHINES.contains(":storedForm \"rejected\""));
        assert!(MACHINES.contains(":AccessRequestMachine_Revoked a"));
        assert!(MACHINES.contains(":storedForm \"revoked\""));
    }

    /// Lines that assign the `state` field outside the transition function.
    // spec: MEDULLA-DATAPLAT-VOCAB-R009.1
    #[test]
    fn no_writer_sets_state_outside_apply() {
        let sources = [include_str!(
            "../../medulla-catalog-types/src/data_product.rs"
        )];
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
