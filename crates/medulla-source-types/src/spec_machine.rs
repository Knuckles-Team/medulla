//! MEDULLA-SDLC-R030.4 -- the requirement-row state as one explicit state machine.
//!
//! `SpecState::apply` is the one transition function. No state is terminal: a row's state is derived from
//! its evidence, so withdrawn evidence moves it back.

use medulla_types::transition_refused::{
    transition, Edge, MachineDef, MachineEvent, MachineState, StateMachineApply, TransitionRefused,
};

use crate::lifecycle::SpecState;

/// An event of `SpecMachine`: the verb of the code path that moves the state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SpecEvent {
    /// A landing change names the row.
    Land,
    /// A test binds the landed row.
    Verify,
    /// The landing evidence is withdrawn.
    Withdraw,
    /// The binding test is gone.
    Unbind,
}

impl MachineState for SpecState {
    fn stored(self) -> &'static str {
        match self {
            Self::Specified => "specified",
            Self::Landed => "landed",
            Self::Verified => "verified",
        }
    }
}

impl MachineEvent for SpecEvent {
    fn name(self) -> &'static str {
        match self {
            Self::Land => "Land",
            Self::Verify => "Verify",
            Self::Withdraw => "Withdraw",
            Self::Unbind => "Unbind",
        }
    }
}

/// `SpecMachine`: the closed edge table. Every other pair of state and event is refused.
pub static SPEC_MACHINE: MachineDef<SpecState, SpecEvent, &'static str> = MachineDef {
    name: "SpecMachine",
    states: &[SpecState::Specified, SpecState::Landed, SpecState::Verified],
    initial: SpecState::Specified,
    terminals: &[],
    events: &[
        SpecEvent::Land,
        SpecEvent::Verify,
        SpecEvent::Withdraw,
        SpecEvent::Unbind,
    ],
    guards: &[],
    edges: &[
        Edge {
            id: "S01",
            from: SpecState::Specified,
            event: SpecEvent::Land,
            guard: None,
            to: SpecState::Landed,
        },
        Edge {
            id: "S02",
            from: SpecState::Landed,
            event: SpecEvent::Verify,
            guard: None,
            to: SpecState::Verified,
        },
        Edge {
            id: "S03",
            from: SpecState::Landed,
            event: SpecEvent::Withdraw,
            guard: None,
            to: SpecState::Specified,
        },
        Edge {
            id: "S04",
            from: SpecState::Verified,
            event: SpecEvent::Withdraw,
            guard: None,
            to: SpecState::Specified,
        },
        Edge {
            id: "S05",
            from: SpecState::Verified,
            event: SpecEvent::Unbind,
            guard: None,
            to: SpecState::Landed,
        },
    ],
};

impl SpecState {
    /// The state a new record starts in.
    pub fn initial() -> Self {
        SPEC_MACHINE.initial
    }

    /// The one transition function of `SpecMachine`.
    pub fn apply(self, event: SpecEvent) -> Result<Self, TransitionRefused> {
        transition(&SPEC_MACHINE, self, event, |_| false).map(|edge| edge.to)
    }
}

impl StateMachineApply for SpecState {
    type Event = SpecEvent;
    type Facts = ();
    fn apply(&self, event: SpecEvent, _facts: &()) -> Result<Self, TransitionRefused> {
        SpecState::apply(*self, event)
    }
}

impl SpecState {
    /// The state a row's evidence puts it in: every record is built by applying events to
    /// the initial state, never by assigning a variant.
    pub fn derive(landed: bool, verified: bool) -> Self {
        let mut state = Self::initial();
        if landed {
            state = state
                .apply(SpecEvent::Land)
                .expect("a specified row accepts Land");
            if verified {
                state = state
                    .apply(SpecEvent::Verify)
                    .expect("a landed row accepts Verify");
            }
        }
        state
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_types::transition_refused::TransitionRefusalReason;

    // spec: MEDULLA-SDLC-R030.4
    #[test]
    fn derive_walks_the_edge_table() {
        assert_eq!(SpecState::derive(false, true), SpecState::Specified);
        assert_eq!(SpecState::derive(true, false), SpecState::Landed);
        assert_eq!(SpecState::derive(true, true), SpecState::Verified);
    }

    // spec: MEDULLA-SDLC-R030.4
    #[test]
    fn every_legal_transition_applies() {
        let machine = &SPEC_MACHINE;
        assert_eq!(machine.edges.len(), 5);
        for edge in machine.edges {
            assert_eq!(edge.from.apply(edge.event), Ok(edge.to), "{}", edge.id);
        }
        let mut state = SpecState::initial();
        for event in [SpecEvent::Land, SpecEvent::Verify] {
            state = state.apply(event).unwrap();
        }
        assert_eq!(state, SpecState::Verified);
    }

    // spec: MEDULLA-SDLC-R030.4
    #[test]
    fn every_illegal_pair_is_refused_with_transition_refused() {
        let machine = &SPEC_MACHINE;
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
                assert_eq!(refused.machine, "SpecMachine");
                assert_eq!(refused.from, state.stored());
                assert_eq!(refused.event, event.name());
                assert_eq!(refused.code().as_str(), "TRANSITION_REFUSED");
            }
        }
        assert_eq!(illegal, 3 * 4 - 5);
    }

    /// Production lines that assign the state outside `apply`: every record is built
    /// from `SpecState::initial()` or an `apply` result, never from a bare variant.
    // spec: MEDULLA-SDLC-R030.4
    #[test]
    fn no_writer_sets_state_outside_apply() {
        for source in [include_str!("../../medulla-core/src/lifecycle/status.rs")] {
            let production = source.split("#[cfg(test)]").next().unwrap();
            let writes: Vec<&str> = production
                .lines()
                .map(str::trim)
                .filter(|l| !l.starts_with("//"))
                .filter(|l| {
                    SPEC_MACHINE.states.iter().any(|s| {
                        let variant = format!("SpecState::{s:?}");
                        l.contains(&format!("state: {variant}"))
                            || l.contains(&format!(".state = {variant}"))
                    })
                })
                .collect();
            assert!(writes.is_empty(), "{writes:?}");
        }
    }
}
