//! The exact-retirement machine `ExactRetirementMachine`
//! (MEDULLA-DURABLE-KERNEL-R874.6, organization constitution Article XXXI).
//!
//! [`ExactRetirementPhase`] is the closed state set, [`ExactRetirementEvent`] the closed
//! event set, [`EXACT_RETIREMENT_MACHINE`] the edge table and
//! [`ExactRetirementPhase::apply`] the one transition function. A retirement starts
//! `Live`, is `Quarantined` by renaming the public name away, and ends `Unlinked`.

use medulla_types::transition_refused::{
    transition, Edge, MachineDef, MachineEvent, MachineState, TransitionRefused,
};

use super::filesystem::ExactRetirementPhase;

/// The closed event set of the exact-retirement machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExactRetirementEvent {
    /// The live name is renamed to its quarantine name.
    Quarantine,
    /// The quarantine name is unlinked.
    Unlink,
}

impl MachineEvent for ExactRetirementEvent {
    fn name(self) -> &'static str {
        match self {
            Self::Quarantine => "Quarantine",
            Self::Unlink => "Unlink",
        }
    }
}

impl MachineState for ExactRetirementPhase {
    fn stored(self) -> &'static str {
        match self {
            Self::Live => "live",
            Self::Quarantined => "quarantined",
            Self::Unlinked => "unlinked",
        }
    }
}

/// The exact-retirement machine has no guards.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExactRetirementGuard {}

/// The exact-retirement machine: `Live` start; `Unlinked` is terminal.
pub const EXACT_RETIREMENT_MACHINE: MachineDef<
    ExactRetirementPhase,
    ExactRetirementEvent,
    ExactRetirementGuard,
> = MachineDef {
    name: "ExactRetirementMachine",
    states: &[
        ExactRetirementPhase::Live,
        ExactRetirementPhase::Quarantined,
        ExactRetirementPhase::Unlinked,
    ],
    initial: ExactRetirementPhase::Live,
    terminals: &[ExactRetirementPhase::Unlinked],
    events: &[
        ExactRetirementEvent::Quarantine,
        ExactRetirementEvent::Unlink,
    ],
    guards: &[],
    edges: &[
        Edge {
            id: "X01",
            from: ExactRetirementPhase::Live,
            event: ExactRetirementEvent::Quarantine,
            guard: None,
            to: ExactRetirementPhase::Quarantined,
        },
        Edge {
            id: "X02",
            from: ExactRetirementPhase::Quarantined,
            event: ExactRetirementEvent::Unlink,
            guard: None,
            to: ExactRetirementPhase::Unlinked,
        },
    ],
};

impl ExactRetirementPhase {
    /// The one transition function: the next phase, or the typed refusal.
    pub fn apply(self, event: ExactRetirementEvent) -> Result<Self, TransitionRefused> {
        transition(
            &EXACT_RETIREMENT_MACHINE,
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

    // spec: MEDULLA-DURABLE-KERNEL-R874.6
    #[test]
    fn every_legal_transition_applies() {
        for edge in EXACT_RETIREMENT_MACHINE.edges {
            assert_eq!(edge.from.apply(edge.event), Ok(edge.to), "{}", edge.id);
        }
    }

    // spec: MEDULLA-DURABLE-KERNEL-R874.6
    #[test]
    fn every_illegal_pair_is_refused_with_transition_refused() {
        let machine = &EXACT_RETIREMENT_MACHINE;
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
                assert_eq!(refused.machine, "ExactRetirementMachine");
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

    // spec: MEDULLA-DURABLE-KERNEL-R874.6
    #[test]
    fn no_writer_sets_phase_outside_apply() {
        let source = include_str!("filesystem.rs");
        let production = source.split("#[cfg(test)]").next().unwrap();
        let writes: Vec<&str> = production
            .lines()
            .map(str::trim)
            .filter(|line| {
                !line.starts_with("//")
                    && line.contains(".phase = ")
                    && !line.contains(".phase.apply(")
            })
            .collect();
        assert!(writes.is_empty(), "{writes:?}");
    }
}
