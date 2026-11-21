//! MEDULLA-UNIFIED-DATA-PLANE-R768.6: the lifecycle of one served modality record as an explicit
//! state machine (organization constitution Article XXXI).
//!
//! A new record is `Active`. `TransitionLifecycle` moves it between `Active` and `Cold`;
//! `Delete` tombstones it from either. `Tombstoned` leaves no edge. [`LifecycleState::apply`]
//! is the only function that produces a new state: the two writers in `mutation.rs`
//! (`ServedModalityRuntime::delete` and `ServedModalityRuntime::transition_lifecycle`) assign its
//! result, and a refused event is the typed `TRANSITION_REFUSED`. The ontology description is
//! `LifecycleMachine` in `state-machines-v1.ttl`.

use super::records::LifecycleState;
use medulla_types::transition_refused::{
    transition, Edge, MachineDef, MachineEvent, MachineState, TransitionRefused,
};

impl MachineState for LifecycleState {
    fn stored(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Cold => "cold",
            Self::Tombstoned => "tombstoned",
        }
    }
}

/// The closed event set of `LifecycleMachine`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LifecycleEvent {
    /// The `Delete` event.
    Delete,
    /// The `TransitionLifecycle` event.
    TransitionLifecycle,
}

impl MachineEvent for LifecycleEvent {
    fn name(self) -> &'static str {
        match self {
            Self::Delete => "Delete",
            Self::TransitionLifecycle => "TransitionLifecycle",
        }
    }
}

/// `LifecycleMachine`: the closed edge table. Every other pair of state and event is refused.
pub static LIFECYCLE_MACHINE: MachineDef<LifecycleState, LifecycleEvent, &'static str> =
    MachineDef {
        name: "LifecycleMachine",
        states: &[
            LifecycleState::Active,
            LifecycleState::Cold,
            LifecycleState::Tombstoned,
        ],
        initial: LifecycleState::Active,
        terminals: &[LifecycleState::Tombstoned],
        events: &[LifecycleEvent::Delete, LifecycleEvent::TransitionLifecycle],
        guards: &[],
        edges: &[
            Edge {
                id: "L01",
                from: LifecycleState::Active,
                event: LifecycleEvent::TransitionLifecycle,
                guard: None,
                to: LifecycleState::Cold,
            },
            Edge {
                id: "L02",
                from: LifecycleState::Cold,
                event: LifecycleEvent::TransitionLifecycle,
                guard: None,
                to: LifecycleState::Active,
            },
            Edge {
                id: "L03",
                from: LifecycleState::Active,
                event: LifecycleEvent::Delete,
                guard: None,
                to: LifecycleState::Tombstoned,
            },
            Edge {
                id: "L04",
                from: LifecycleState::Cold,
                event: LifecycleEvent::Delete,
                guard: None,
                to: LifecycleState::Tombstoned,
            },
        ],
    };

impl LifecycleState {
    /// The one transition function of `LifecycleMachine`.
    pub fn apply(self, event: LifecycleEvent) -> Result<Self, TransitionRefused> {
        transition(&LIFECYCLE_MACHINE, self, event, |_| false).map(|edge| edge.to)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_types::transition_refused::TransitionRefusalReason;

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R768.6
    #[test]
    fn every_legal_transition_applies() {
        assert_eq!(LIFECYCLE_MACHINE.edges.len(), 4);
        for edge in LIFECYCLE_MACHINE.edges {
            assert_eq!(edge.from.apply(edge.event), Ok(edge.to), "edge {}", edge.id);
        }
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R768.6
    #[test]
    fn every_illegal_pair_is_refused_with_transition_refused() {
        let machine = &LIFECYCLE_MACHINE;
        let mut refused_pairs = 0;
        for &state in machine.states {
            for &event in machine.events {
                if machine
                    .edges
                    .iter()
                    .any(|edge| edge.from == state && edge.event == event)
                {
                    continue;
                }
                let refused = state.apply(event).unwrap_err();
                assert_eq!(refused.machine, "LifecycleMachine");
                assert_eq!(refused.from, state.stored());
                assert_eq!(refused.event, event.name());
                assert_eq!(refused.reason, TransitionRefusalReason::TerminalState);
                assert_eq!(refused.code().as_str(), "TRANSITION_REFUSED");
                refused_pairs += 1;
            }
        }
        assert_eq!(refused_pairs, 2);
    }

    /// The two writers in `mutation.rs` assign the result of `apply` and nothing else does.
    // spec: MEDULLA-UNIFIED-DATA-PLANE-R768.6
    #[test]
    fn no_writer_sets_lifecycle_outside_apply() {
        let source = include_str!("mutation.rs");
        let production = source.split("#[cfg(test)]").next().unwrap();
        let lines = || {
            production
                .lines()
                .map(str::trim)
                .filter(|line| !line.starts_with("//"))
        };
        assert_eq!(lines().filter(|l| l.contains(".lifecycle = ")).count(), 2);
        assert_eq!(
            lines()
                .filter(|l| l.starts_with(".apply(LifecycleEvent::"))
                .count(),
            2
        );
        assert!(lines().all(|l| !l.contains(".lifecycle = LifecycleState::")));
    }
}
