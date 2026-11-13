//! MEDULLA-DECISION-ENGINE-R036.106: the lifecycle of one durable profile-use grant as an
//! explicit state machine.
//!
//! `Active` is the initial state; the closed event set is `Revoke`, which enters the terminal
//! `Revoked`. [`ProfileGrantState::apply`] is the only function that produces a new state, and a
//! refused event is the typed `TRANSITION_REFUSED` naming the machine, the current state and the
//! event. The ontology description is `ProfileGrantMachine` in `state-machines-v1.ttl`.

use serde::{Deserialize, Serialize};

use crate::transition_refused::{
    transition, Edge, MachineDef, MachineEvent, MachineState, TransitionRefused,
};

/// The state of one profile-use grant: the closed state set of `ProfileGrantMachine`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileGrantState {
    /// The principal may use the profile.
    Active,
    /// The grant was withdrawn; it never becomes active again.
    Revoked,
}

/// The closed event set of `ProfileGrantMachine`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ProfileGrantEvent {
    /// The grant is withdrawn.
    Revoke,
}

impl MachineState for ProfileGrantState {
    fn stored(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Revoked => "revoked",
        }
    }
}

impl MachineEvent for ProfileGrantEvent {
    fn name(self) -> &'static str {
        match self {
            Self::Revoke => "Revoke",
        }
    }
}

/// `ProfileGrantMachine`: the closed edge table. Every other pair is refused.
pub static PROFILE_GRANT_MACHINE: MachineDef<ProfileGrantState, ProfileGrantEvent, &'static str> =
    MachineDef {
        name: "ProfileGrantMachine",
        states: &[ProfileGrantState::Active, ProfileGrantState::Revoked],
        initial: ProfileGrantState::Active,
        terminals: &[ProfileGrantState::Revoked],
        events: &[ProfileGrantEvent::Revoke],
        guards: &[],
        edges: &[Edge {
            id: "G01",
            from: ProfileGrantState::Active,
            event: ProfileGrantEvent::Revoke,
            guard: None,
            to: ProfileGrantState::Revoked,
        }],
    };

impl ProfileGrantState {
    /// The one transition function of `ProfileGrantMachine`.
    pub fn apply(self, event: ProfileGrantEvent) -> Result<Self, TransitionRefused> {
        transition(&PROFILE_GRANT_MACHINE, self, event, |_| false).map(|edge| edge.to)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transition_refused::TransitionRefusalReason;

    #[test]
    fn revoke_is_the_one_legal_edge_and_a_second_revoke_is_refused() {
        assert_eq!(
            ProfileGrantState::Active.apply(ProfileGrantEvent::Revoke),
            Ok(ProfileGrantState::Revoked)
        );
        let refused = ProfileGrantState::Revoked
            .apply(ProfileGrantEvent::Revoke)
            .unwrap_err();
        assert_eq!(refused.machine, "ProfileGrantMachine");
        assert_eq!(refused.from, "revoked");
        assert_eq!(refused.event, "Revoke");
        assert_eq!(refused.reason, TransitionRefusalReason::TerminalState);
    }

    const MACHINES: &str = include_str!("../../medulla-core/ontology/state-machines-v1.ttl");

    #[test]
    fn the_ontology_describes_the_same_machine() {
        assert!(MACHINES.contains(":ProfileGrantMachine a :StateMachine"));
        assert!(MACHINES.contains(":ProfileGrantMachine_Active a"));
        assert!(MACHINES.contains(":ProfileGrantMachine_Revoked a :ProfileGrantTerminalState"));
        assert!(MACHINES.contains(":storedForm \"active\""));
        assert!(MACHINES.contains(":storedForm \"revoked\""));
        assert!(MACHINES.contains(":ProfileGrantMachine_Event_Revoke a"));
        let start = MACHINES
            .find(":ProfileGrantMachine_G01 a :ProfileGrantTransition")
            .expect("edge declared");
        let text = &MACHINES[start..];
        let text = &text[..text.find(" .\n").unwrap()];
        assert!(text.contains(":fromState :ProfileGrantMachine_Active"));
        assert!(text.contains(":toState :ProfileGrantMachine_Revoked"));
        assert!(text.contains(":firedBy :ProfileGrantMachine_Event_Revoke"));
    }
}
