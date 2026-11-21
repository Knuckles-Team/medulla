//! The participant-transaction machine `ParticipantTransactionMachine`
//! (MEDULLA-DURABLE-KERNEL-R874.15, organization constitution Article XXXI).
//!
//! [`ParticipantTransactionState`] carries a decision, so the table runs over the
//! data-free [`ParticipantTransactionKind`]. [`ParticipantTransactionEvent`] is the closed
//! event set, [`PARTICIPANT_TRANSACTION_MACHINE`] the edge table and
//! [`ParticipantTransactionKind::apply`] the one transition function. A participant starts
//! `Prepared`, resolves to the parent's decision, and is then collected; the two collected
//! kinds are terminal.

use medulla_types::transition_refused::{
    transition, Edge, MachineDef, MachineEvent, MachineState, TransitionRefused,
};

use crate::consensus::{ParticipantTransactionState, TransactionDecision};

/// The data-free kind of a [`ParticipantTransactionState`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParticipantTransactionKind {
    /// Prepared, unresolved.
    Prepared,
    /// Resolved to commit.
    ResolvedCommit,
    /// Resolved to abort.
    ResolvedAbort,
    /// Collected after a commit resolution.
    CollectedCommit,
    /// Collected after an abort resolution.
    CollectedAbort,
}

impl ParticipantTransactionState {
    /// The kind of this state.
    pub fn kind(self) -> ParticipantTransactionKind {
        match self {
            Self::Prepared => ParticipantTransactionKind::Prepared,
            Self::Resolved {
                decision: TransactionDecision::Commit,
            } => ParticipantTransactionKind::ResolvedCommit,
            Self::Resolved {
                decision: TransactionDecision::Abort,
            } => ParticipantTransactionKind::ResolvedAbort,
            Self::Collected {
                decision: TransactionDecision::Commit,
            } => ParticipantTransactionKind::CollectedCommit,
            Self::Collected {
                decision: TransactionDecision::Abort,
            } => ParticipantTransactionKind::CollectedAbort,
        }
    }
}

impl MachineState for ParticipantTransactionKind {
    fn stored(self) -> &'static str {
        match self {
            Self::Prepared => "prepared",
            Self::ResolvedCommit => "resolved_commit",
            Self::ResolvedAbort => "resolved_abort",
            Self::CollectedCommit => "collected_commit",
            Self::CollectedAbort => "collected_abort",
        }
    }
}

/// The closed event set of the participant-transaction machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParticipantTransactionEvent {
    /// The participant resolves to a commit decision.
    ResolveCommit,
    /// The participant resolves to an abort decision.
    ResolveAbort,
    /// The commit-resolved participant is collected.
    CollectCommit,
    /// The abort-resolved participant is collected.
    CollectAbort,
}

impl ParticipantTransactionEvent {
    /// The event that would move a participant into a state of kind `next`, or `None`
    /// for `Prepared`, which no event reaches.
    pub fn reaching(next: ParticipantTransactionKind) -> Option<Self> {
        match next {
            ParticipantTransactionKind::Prepared => None,
            ParticipantTransactionKind::ResolvedCommit => Some(Self::ResolveCommit),
            ParticipantTransactionKind::ResolvedAbort => Some(Self::ResolveAbort),
            ParticipantTransactionKind::CollectedCommit => Some(Self::CollectCommit),
            ParticipantTransactionKind::CollectedAbort => Some(Self::CollectAbort),
        }
    }
}

impl MachineEvent for ParticipantTransactionEvent {
    fn name(self) -> &'static str {
        match self {
            Self::ResolveCommit => "ResolveCommit",
            Self::ResolveAbort => "ResolveAbort",
            Self::CollectCommit => "CollectCommit",
            Self::CollectAbort => "CollectAbort",
        }
    }
}

/// The participant-transaction machine has no guards.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParticipantTransactionGuard {}

/// The participant-transaction machine: `Prepared` start; both collected kinds are
/// terminal.
pub const PARTICIPANT_TRANSACTION_MACHINE: MachineDef<
    ParticipantTransactionKind,
    ParticipantTransactionEvent,
    ParticipantTransactionGuard,
> = MachineDef {
    name: "ParticipantTransactionMachine",
    states: &[
        ParticipantTransactionKind::Prepared,
        ParticipantTransactionKind::ResolvedCommit,
        ParticipantTransactionKind::ResolvedAbort,
        ParticipantTransactionKind::CollectedCommit,
        ParticipantTransactionKind::CollectedAbort,
    ],
    initial: ParticipantTransactionKind::Prepared,
    terminals: &[
        ParticipantTransactionKind::CollectedCommit,
        ParticipantTransactionKind::CollectedAbort,
    ],
    events: &[
        ParticipantTransactionEvent::ResolveCommit,
        ParticipantTransactionEvent::ResolveAbort,
        ParticipantTransactionEvent::CollectCommit,
        ParticipantTransactionEvent::CollectAbort,
    ],
    guards: &[],
    edges: &[
        Edge {
            id: "T01",
            from: ParticipantTransactionKind::Prepared,
            event: ParticipantTransactionEvent::ResolveCommit,
            guard: None,
            to: ParticipantTransactionKind::ResolvedCommit,
        },
        Edge {
            id: "T02",
            from: ParticipantTransactionKind::Prepared,
            event: ParticipantTransactionEvent::ResolveAbort,
            guard: None,
            to: ParticipantTransactionKind::ResolvedAbort,
        },
        Edge {
            id: "T03",
            from: ParticipantTransactionKind::ResolvedCommit,
            event: ParticipantTransactionEvent::CollectCommit,
            guard: None,
            to: ParticipantTransactionKind::CollectedCommit,
        },
        Edge {
            id: "T04",
            from: ParticipantTransactionKind::ResolvedAbort,
            event: ParticipantTransactionEvent::CollectAbort,
            guard: None,
            to: ParticipantTransactionKind::CollectedAbort,
        },
    ],
};

impl ParticipantTransactionKind {
    /// The one transition function: the next kind, or the typed refusal.
    pub fn apply(self, event: ParticipantTransactionEvent) -> Result<Self, TransitionRefused> {
        transition(&PARTICIPANT_TRANSACTION_MACHINE, self, event, |guard| {
            match guard {}
        })
        .map(|edge| edge.to)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_types::transition_refused::TransitionRefusalReason;

    // spec: MEDULLA-DURABLE-KERNEL-R874.15
    #[test]
    fn every_legal_transition_applies() {
        for edge in PARTICIPANT_TRANSACTION_MACHINE.edges {
            assert_eq!(edge.from.apply(edge.event), Ok(edge.to), "{}", edge.id);
        }
        for decision in [TransactionDecision::Commit, TransactionDecision::Abort] {
            let resolved = ParticipantTransactionState::Resolved { decision };
            let collected = ParticipantTransactionState::Collected { decision };
            assert!(ParticipantTransactionState::Prepared
                .validate_successor(resolved)
                .is_ok());
            assert!(resolved.validate_successor(collected).is_ok());
            assert!(resolved.validate_successor(resolved).is_ok(), "a replay");
        }
    }

    // spec: MEDULLA-DURABLE-KERNEL-R874.15
    #[test]
    fn every_illegal_pair_is_refused_with_transition_refused() {
        let machine = &PARTICIPANT_TRANSACTION_MACHINE;
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
                assert_eq!(refused.machine, "ParticipantTransactionMachine");
                assert_eq!(refused.from, state.stored());
                assert_eq!(refused.event, event.name());
                assert_eq!(refused.code().as_str(), "TRANSITION_REFUSED");
            }
        }
        assert_eq!(
            illegal,
            machine.states.len() * machine.events.len() - machine.edges.len()
        );
        // A decision cannot change between resolved and collected.
        let resolved = ParticipantTransactionState::Resolved {
            decision: TransactionDecision::Commit,
        };
        let flipped = ParticipantTransactionState::Collected {
            decision: TransactionDecision::Abort,
        };
        assert!(resolved.validate_successor(flipped).is_err());
        assert!(ParticipantTransactionState::Prepared
            .validate_successor(flipped)
            .is_err());
    }

    // spec: MEDULLA-DURABLE-KERNEL-R874.15
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
        assert!(
            ParticipantTransactionEvent::reaching(ParticipantTransactionKind::Prepared).is_none()
        );
    }
}
