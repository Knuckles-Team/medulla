//! The mutation-batch machine `MutationBatchMachine`
//! (MEDULLA-DURABLE-KERNEL-R874.10, organization constitution Article XXXI).
//!
//! [`MutationBatchStatus`] is the closed state set, [`MutationBatchEvent`] the closed event
//! set, [`MUTATION_BATCH_MACHINE`] the edge table and [`MutationBatchStatus::apply`] the one
//! transition function. A saga record starts `Prepared`; the commit event makes it
//! `Committed`, which is terminal. No production code writes any other status, so the
//! machine has no edge into another state.

use medulla_types::transition_refused::{
    transition, Edge, MachineDef, MachineEvent, MachineState, TransitionRefused,
};

use super::MutationBatchStatus;

/// The closed event set of the mutation-batch machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MutationBatchEvent {
    /// The batch committed.
    Commit,
}

impl MachineEvent for MutationBatchEvent {
    fn name(self) -> &'static str {
        match self {
            Self::Commit => "Commit",
        }
    }
}

impl MachineState for MutationBatchStatus {
    fn stored(self) -> &'static str {
        match self {
            Self::Prepared => "prepared",
            Self::Committed => "committed",
        }
    }
}

/// The mutation-batch machine has no guards.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MutationBatchGuard {}

/// The mutation-batch machine: `Prepared` start; `Committed` is terminal.
pub const MUTATION_BATCH_MACHINE: MachineDef<
    MutationBatchStatus,
    MutationBatchEvent,
    MutationBatchGuard,
> = MachineDef {
    name: "MutationBatchMachine",
    states: &[
        MutationBatchStatus::Prepared,
        MutationBatchStatus::Committed,
    ],
    initial: MutationBatchStatus::Prepared,
    terminals: &[MutationBatchStatus::Committed],
    events: &[MutationBatchEvent::Commit],
    guards: &[],
    edges: &[Edge {
        id: "B01",
        from: MutationBatchStatus::Prepared,
        event: MutationBatchEvent::Commit,
        guard: None,
        to: MutationBatchStatus::Committed,
    }],
};

impl MutationBatchStatus {
    /// The one transition function: the next status, or the typed refusal.
    pub fn apply(self, event: MutationBatchEvent) -> Result<Self, TransitionRefused> {
        transition(&MUTATION_BATCH_MACHINE, self, event, |guard| match guard {}).map(|edge| edge.to)
    }

    /// The status of a one-shot commit: the initial status advanced by the commit event,
    /// so the record passes through `Prepared` without storing it.
    pub fn committed() -> Self {
        MUTATION_BATCH_MACHINE
            .initial
            .apply(MutationBatchEvent::Commit)
            .unwrap_or(Self::Committed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_types::transition_refused::TransitionRefusalReason;

    // spec: MEDULLA-DURABLE-KERNEL-R874.10
    #[test]
    fn every_legal_transition_applies() {
        for edge in MUTATION_BATCH_MACHINE.edges {
            assert_eq!(edge.from.apply(edge.event), Ok(edge.to), "{}", edge.id);
        }
        assert_eq!(
            MutationBatchStatus::committed(),
            MutationBatchStatus::Committed
        );
    }

    // spec: MEDULLA-DURABLE-KERNEL-R874.10
    #[test]
    fn every_illegal_pair_is_refused_with_transition_refused() {
        let machine = &MUTATION_BATCH_MACHINE;
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
                assert_eq!(refused.reason, TransitionRefusalReason::TerminalState);
                assert_eq!(refused.machine, "MutationBatchMachine");
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

    // spec: MEDULLA-DURABLE-KERNEL-R874.10
    #[test]
    fn no_writer_sets_status_outside_apply() {
        for (name, source) in [
            (
                "medulla-transaction/src/saga.rs",
                include_str!("../../../../medulla-transaction/src/saga.rs"),
            ),
            (
                "medulla-transaction/src/commit.rs",
                include_str!("../../../../medulla-transaction/src/commit.rs"),
            ),
        ] {
            let production = source.split("#[cfg(test)]").next().unwrap();
            let writes: Vec<&str> = production
                .lines()
                .map(str::trim)
                .filter(|line| {
                    !line.starts_with("//")
                        && (line.starts_with("status: MutationBatchStatus::")
                            || line.contains(".status = "))
                })
                .collect();
            assert!(writes.is_empty(), "{name}: {writes:?}");
        }
    }
}
