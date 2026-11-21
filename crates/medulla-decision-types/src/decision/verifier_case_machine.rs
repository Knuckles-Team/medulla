//! MEDULLA-DECISION-ENGINE-R831.1 -- the state of a verifier attack case as one explicit
//! state machine (organization constitution Article XXXI).
//!
//! A case is `Open` when an attack found a defect, `Fixed` once a fix is proposed, `Retested`
//! when the retest held against the proposed artifact and `Failed` when it did not. `Retested`
//! is terminal for its case. [`VerifierCaseState::apply`] is the one transition function: it is
//! the only code that moves a case between states, and it returns the typed
//! [`TransitionRefused`] for every pair the edge table does not list. The one parse site builds
//! the initial state with [`VerifierCaseState::from_word`].

use medulla_types::transition_refused::{
    transition, Edge, MachineDef, MachineEvent, MachineState, StateMachineApply, TransitionRefused,
};

use crate::decision::verifier_receipt::VerifierCaseState;

/// The closed event set of `VerifierCaseMachine`: one event per legal edge.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum VerifierCaseEvent {
    /// A fix was proposed for the defect.
    ProposeFix,
    /// The retest against the proposed artifact held.
    RetestHeld,
    /// The retest against the proposed artifact failed.
    RetestFailed,
    /// A new attack found the defect again.
    Reopen,
}

/// The facts the guard of `VerifierCaseMachine` reads.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct VerifierCaseFacts {
    /// The case names the artifact its retest ran against (guard `HasRetestDigest`).
    pub has_retest_digest: bool,
}

impl MachineState for VerifierCaseState {
    fn stored(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Fixed => "fixed",
            Self::Retested => "retested",
            Self::Failed => "failed",
        }
    }
}

impl MachineEvent for VerifierCaseEvent {
    fn name(self) -> &'static str {
        match self {
            Self::ProposeFix => "ProposeFix",
            Self::RetestHeld => "RetestHeld",
            Self::RetestFailed => "RetestFailed",
            Self::Reopen => "Reopen",
        }
    }
}

/// Guard of `VerifierCaseMachine`: the case names the artifact its retest ran against.
pub const VERIFIER_CASE_GUARD_HAS_RETEST_DIGEST: &str = "HasRetestDigest";

/// `VerifierCaseMachine`: the closed edge table. Every other pair of state and event is refused.
pub static VERIFIER_CASE_MACHINE: MachineDef<VerifierCaseState, VerifierCaseEvent, &'static str> =
    MachineDef {
        name: "VerifierCaseMachine",
        states: &[
            VerifierCaseState::Open,
            VerifierCaseState::Fixed,
            VerifierCaseState::Retested,
            VerifierCaseState::Failed,
        ],
        initial: VerifierCaseState::Open,
        terminals: &[VerifierCaseState::Retested],
        events: &[
            VerifierCaseEvent::ProposeFix,
            VerifierCaseEvent::RetestHeld,
            VerifierCaseEvent::RetestFailed,
            VerifierCaseEvent::Reopen,
        ],
        guards: &[VERIFIER_CASE_GUARD_HAS_RETEST_DIGEST],
        edges: &[
            Edge {
                id: "C01",
                from: VerifierCaseState::Open,
                event: VerifierCaseEvent::ProposeFix,
                guard: None,
                to: VerifierCaseState::Fixed,
            },
            Edge {
                id: "C02",
                from: VerifierCaseState::Fixed,
                event: VerifierCaseEvent::RetestHeld,
                guard: Some(VERIFIER_CASE_GUARD_HAS_RETEST_DIGEST),
                to: VerifierCaseState::Retested,
            },
            Edge {
                id: "C03",
                from: VerifierCaseState::Fixed,
                event: VerifierCaseEvent::RetestFailed,
                guard: None,
                to: VerifierCaseState::Failed,
            },
            Edge {
                id: "C04",
                from: VerifierCaseState::Failed,
                event: VerifierCaseEvent::ProposeFix,
                guard: None,
                to: VerifierCaseState::Fixed,
            },
            Edge {
                id: "C05",
                from: VerifierCaseState::Open,
                event: VerifierCaseEvent::Reopen,
                guard: None,
                to: VerifierCaseState::Open,
            },
            Edge {
                id: "C06",
                from: VerifierCaseState::Fixed,
                event: VerifierCaseEvent::Reopen,
                guard: None,
                to: VerifierCaseState::Open,
            },
            Edge {
                id: "C07",
                from: VerifierCaseState::Failed,
                event: VerifierCaseEvent::Reopen,
                guard: None,
                to: VerifierCaseState::Open,
            },
        ],
    };

impl VerifierCaseState {
    /// The state a newly attacked case takes: the initial state of `VerifierCaseMachine`.
    pub fn initial() -> Self {
        VERIFIER_CASE_MACHINE.initial
    }

    /// The state a stored word names. The set is closed: any other word is refused.
    pub fn from_word(word: &str) -> Result<Self, String> {
        VERIFIER_CASE_MACHINE
            .state_from_stored(word)
            .ok_or_else(|| format!("{word:?} is not a verifier case state"))
    }

    /// The one transition function of `VerifierCaseMachine`. The guard reads `facts`.
    pub fn apply(
        self,
        event: VerifierCaseEvent,
        facts: VerifierCaseFacts,
    ) -> Result<Self, TransitionRefused> {
        transition(&VERIFIER_CASE_MACHINE, self, event, |guard| match guard {
            VERIFIER_CASE_GUARD_HAS_RETEST_DIGEST => facts.has_retest_digest,
            _ => false,
        })
        .map(|edge| edge.to)
    }
}

impl StateMachineApply for VerifierCaseState {
    type Event = VerifierCaseEvent;
    type Facts = VerifierCaseFacts;

    fn apply(
        &self,
        event: VerifierCaseEvent,
        facts: &VerifierCaseFacts,
    ) -> Result<Self, TransitionRefused> {
        VerifierCaseState::apply(*self, event, *facts)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_types::transition_refused::TransitionRefusalReason;

    const HELD: VerifierCaseFacts = VerifierCaseFacts {
        has_retest_digest: true,
    };

    // spec: MEDULLA-DECISION-ENGINE-R831.1
    #[test]
    fn every_legal_transition_applies() {
        for edge in VERIFIER_CASE_MACHINE.edges {
            assert_eq!(edge.from.apply(edge.event, HELD), Ok(edge.to), "{}", edge.id);
            if edge.guard.is_some() {
                let refused = edge
                    .from
                    .apply(edge.event, VerifierCaseFacts::default())
                    .unwrap_err();
                assert_eq!(refused.reason, TransitionRefusalReason::GuardFailed);
            }
        }
        assert_eq!(VerifierCaseState::initial(), VerifierCaseState::Open);
    }

    // spec: MEDULLA-DECISION-ENGINE-R831.1
    #[test]
    fn every_illegal_pair_is_refused_with_transition_refused() {
        let machine = &VERIFIER_CASE_MACHINE;
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
                let refused = state.apply(event, HELD).unwrap_err();
                let expected = if machine.is_terminal(state) {
                    TransitionRefusalReason::TerminalState
                } else {
                    TransitionRefusalReason::IllegalTransition
                };
                assert_eq!(refused.reason, expected);
                assert_eq!(refused.machine, "VerifierCaseMachine");
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

    // spec: MEDULLA-DECISION-ENGINE-R831.1
    #[test]
    fn the_word_parse_is_closed() {
        for state in VERIFIER_CASE_MACHINE.states {
            assert_eq!(VerifierCaseState::from_word(state.stored()), Ok(*state));
        }
        assert!(VerifierCaseState::from_word("reopened").is_err());
        assert!(VerifierCaseState::from_word("Open").is_err());
    }

    /// No production line assigns a case state from a literal variant.
    // spec: MEDULLA-DECISION-ENGINE-R831.1
    #[test]
    fn no_writer_sets_state_outside_apply() {
        for source in [
            include_str!("../../../medulla-h-decide/src/decide/family/generated_code_admission.rs"),
            include_str!("../../../medulla-decision-stats/src/decision/verifier_receipt.rs"),
            include_str!("verifier_receipt.rs"),
        ] {
            let production = source.split("#[cfg(test)]").next().unwrap();
            let writes: Vec<&str> = production
                .lines()
                .map(str::trim)
                .filter(|line| {
                    !line.starts_with("//")
                        && (line.contains(".state = VerifierCaseState::")
                            || line.contains("state: VerifierCaseState::"))
                })
                .collect();
            assert!(writes.is_empty(), "{writes:?}");
        }
    }
}
