//! The parent-transaction machine `ParentTransactionMachine`
//! (MEDULLA-DURABLE-KERNEL-R874.14, organization constitution Article XXXI).
//!
//! [`ParentTransactionState`] carries a decision, so the table runs over the data-free
//! [`ParentTransactionKind`]. [`ParentTransactionEvent`] is the closed event set,
//! [`PARENT_TRANSACTION_MACHINE`] the edge table and [`ParentTransactionKind::apply`] the
//! one transition function. A parent starts `Prepared`, is decided to commit or abort,
//! and is then finalized; the two finalized kinds are terminal.

use medulla_types::transition_refused::{
    transition, Edge, MachineDef, MachineEvent, MachineState, TransitionRefused,
};

use crate::consensus::{ParentTransactionState, TransactionDecision};

/// The data-free kind of a [`ParentTransactionState`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParentTransactionKind {
    /// Prepared, undecided.
    Prepared,
    /// Decided to commit.
    DecidedCommit,
    /// Decided to abort.
    DecidedAbort,
    /// Finalized after a commit decision.
    FinalizedCommit,
    /// Finalized after an abort decision.
    FinalizedAbort,
}

impl ParentTransactionState {
    /// The kind of this state, as `PartitionState::kind` maps its states.
    pub fn kind(self) -> ParentTransactionKind {
        match self {
            Self::Prepared => ParentTransactionKind::Prepared,
            Self::Decided {
                decision: TransactionDecision::Commit,
            } => ParentTransactionKind::DecidedCommit,
            Self::Decided {
                decision: TransactionDecision::Abort,
            } => ParentTransactionKind::DecidedAbort,
            Self::Finalized {
                decision: TransactionDecision::Commit,
            } => ParentTransactionKind::FinalizedCommit,
            Self::Finalized {
                decision: TransactionDecision::Abort,
            } => ParentTransactionKind::FinalizedAbort,
        }
    }
}

impl MachineState for ParentTransactionKind {
    fn stored(self) -> &'static str {
        match self {
            Self::Prepared => "prepared",
            Self::DecidedCommit => "decided_commit",
            Self::DecidedAbort => "decided_abort",
            Self::FinalizedCommit => "finalized_commit",
            Self::FinalizedAbort => "finalized_abort",
        }
    }
}

/// The closed event set of the parent-transaction machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParentTransactionEvent {
    /// The coordinator decides to commit.
    DecideCommit,
    /// The coordinator decides to abort.
    DecideAbort,
    /// The decided parent is finalized.
    Finalize,
}

impl ParentTransactionEvent {
    /// The event that would move a parent into a state of kind `next`, or `None` for
    /// `Prepared`, which no event reaches.
    pub fn reaching(next: ParentTransactionKind) -> Option<Self> {
        match next {
            ParentTransactionKind::Prepared => None,
            ParentTransactionKind::DecidedCommit => Some(Self::DecideCommit),
            ParentTransactionKind::DecidedAbort => Some(Self::DecideAbort),
            ParentTransactionKind::FinalizedCommit | ParentTransactionKind::FinalizedAbort => {
                Some(Self::Finalize)
            }
        }
    }
}

impl MachineEvent for ParentTransactionEvent {
    fn name(self) -> &'static str {
        match self {
            Self::DecideCommit => "DecideCommit",
            Self::DecideAbort => "DecideAbort",
            Self::Finalize => "Finalize",
        }
    }
}

/// The parent-transaction machine has no guards.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParentTransactionGuard {}

/// The parent-transaction machine: `Prepared` start; both finalized kinds are terminal.
pub const PARENT_TRANSACTION_MACHINE: MachineDef<
    ParentTransactionKind,
    ParentTransactionEvent,
    ParentTransactionGuard,
> = MachineDef {
    name: "ParentTransactionMachine",
    states: &[
        ParentTransactionKind::Prepared,
        ParentTransactionKind::DecidedCommit,
        ParentTransactionKind::DecidedAbort,
        ParentTransactionKind::FinalizedCommit,
        ParentTransactionKind::FinalizedAbort,
    ],
    initial: ParentTransactionKind::Prepared,
    terminals: &[
        ParentTransactionKind::FinalizedCommit,
        ParentTransactionKind::FinalizedAbort,
    ],
    events: &[
        ParentTransactionEvent::DecideCommit,
        ParentTransactionEvent::DecideAbort,
        ParentTransactionEvent::Finalize,
    ],
    guards: &[],
    edges: &[
        Edge {
            id: "P01",
            from: ParentTransactionKind::Prepared,
            event: ParentTransactionEvent::DecideCommit,
            guard: None,
            to: ParentTransactionKind::DecidedCommit,
        },
        Edge {
            id: "P02",
            from: ParentTransactionKind::Prepared,
            event: ParentTransactionEvent::DecideAbort,
            guard: None,
            to: ParentTransactionKind::DecidedAbort,
        },
        Edge {
            id: "P03",
            from: ParentTransactionKind::DecidedCommit,
            event: ParentTransactionEvent::Finalize,
            guard: None,
            to: ParentTransactionKind::FinalizedCommit,
        },
        Edge {
            id: "P04",
            from: ParentTransactionKind::DecidedAbort,
            event: ParentTransactionEvent::Finalize,
            guard: None,
            to: ParentTransactionKind::FinalizedAbort,
        },
    ],
};

impl ParentTransactionKind {
    /// The one transition function: the next kind, or the typed refusal.
    pub fn apply(self, event: ParentTransactionEvent) -> Result<Self, TransitionRefused> {
        transition(
            &PARENT_TRANSACTION_MACHINE,
            self,
            event,
            |guard| match guard {},
        )
        .map(|edge| edge.to)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_types::transition_refused::TransitionRefusalReason;

    // spec: MEDULLA-DURABLE-KERNEL-R874.14
    #[test]
    fn every_legal_transition_applies() {
        for edge in PARENT_TRANSACTION_MACHINE.edges {
            assert_eq!(edge.from.apply(edge.event), Ok(edge.to), "{}", edge.id);
        }
        for decision in [TransactionDecision::Commit, TransactionDecision::Abort] {
            let decided = ParentTransactionState::Decided { decision };
            let finalized = ParentTransactionState::Finalized { decision };
            assert!(ParentTransactionState::Prepared
                .validate_successor(decided)
                .is_ok());
            assert!(decided.validate_successor(finalized).is_ok());
            assert!(decided.validate_successor(decided).is_ok(), "a replay");
        }
    }

    // spec: MEDULLA-DURABLE-KERNEL-R874.14
    #[test]
    fn every_illegal_pair_is_refused_with_transition_refused() {
        let machine = &PARENT_TRANSACTION_MACHINE;
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
                let expected = if machine.terminals.contains(&state) {
                    TransitionRefusalReason::TerminalState
                } else {
                    TransitionRefusalReason::IllegalTransition
                };
                assert_eq!(refused.reason, expected);
                assert_eq!(refused.machine, "ParentTransactionMachine");
                assert_eq!(refused.from, state.stored());
                assert_eq!(refused.event, event.name());
                assert_eq!(refused.code().as_str(), "TRANSITION_REFUSED");
            }
        }
        assert_eq!(
            illegal,
            machine.states.len() * machine.events.len() - machine.edges.len()
        );
        // A decision cannot change between decided and finalized.
        let decided = ParentTransactionState::Decided {
            decision: TransactionDecision::Commit,
        };
        let flipped = ParentTransactionState::Finalized {
            decision: TransactionDecision::Abort,
        };
        assert!(decided.validate_successor(flipped).is_err());
        assert!(ParentTransactionState::Prepared
            .validate_successor(flipped)
            .is_err());
    }

    // spec: MEDULLA-DURABLE-KERNEL-R874.14
    #[test]
    fn no_writer_sets_state_outside_apply() {
        let source = include_str!("consensus.rs");
        let production = source.split("#[cfg(test)]").next().unwrap();
        let writes: Vec<&str> = production
            .lines()
            .map(str::trim)
            .filter(|line| !line.starts_with("//") && line.contains(".state = "))
            .collect();
        assert!(writes.is_empty(), "{writes:?}");
        assert!(ParentTransactionEvent::reaching(ParentTransactionKind::Prepared).is_none());
    }
}
