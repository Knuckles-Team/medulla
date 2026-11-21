//! MEDULLA-ORCH-VOCAB-R815.4 -- the work market Gap's status as one explicit state machine
//! (organization constitution Article XXXI).
//!
//! [`GapStatus::apply`] is the one transition function and `GapView.status` is the one
//! persisted field it writes. No state is terminal: new evidence reopens a closed Gap.

use medulla_types::transition_refused::{
    transition, Edge, MachineDef, MachineEvent, MachineState, StateMachineApply, TransitionRefused,
};

use super::GapStatus;

/// An event of `GapViewMachine`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GapEvent {
    /// A specification is in flight.
    Specify,
    /// The Gap closed on evidence.
    Resolve,
    /// The Gap was parked.
    Defer,
    /// New evidence reopened a closed Gap.
    Reopen,
}

impl MachineState for GapStatus {
    fn stored(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Specified => "specified",
            Self::Resolved => "resolved",
            Self::Deferred => "deferred",
        }
    }
}

impl MachineEvent for GapEvent {
    fn name(self) -> &'static str {
        match self {
            Self::Specify => "specify",
            Self::Resolve => "resolve",
            Self::Defer => "defer",
            Self::Reopen => "reopen",
        }
    }
}

/// `GapViewMachine`: the closed edge table. Every other pair of state and event is refused.
pub static GAP_VIEW_MACHINE: MachineDef<GapStatus, GapEvent, &'static str> = MachineDef {
    name: "GapViewMachine",
    states: &[
        GapStatus::Open,
        GapStatus::Specified,
        GapStatus::Resolved,
        GapStatus::Deferred,
    ],
    initial: GapStatus::Open,
    terminals: &[],
    events: &[
        GapEvent::Specify,
        GapEvent::Resolve,
        GapEvent::Defer,
        GapEvent::Reopen,
    ],
    guards: &[],
    edges: &[
        Edge {
            id: "G01",
            from: GapStatus::Open,
            event: GapEvent::Specify,
            guard: None,
            to: GapStatus::Specified,
        },
        Edge {
            id: "G02",
            from: GapStatus::Open,
            event: GapEvent::Resolve,
            guard: None,
            to: GapStatus::Resolved,
        },
        Edge {
            id: "G03",
            from: GapStatus::Open,
            event: GapEvent::Defer,
            guard: None,
            to: GapStatus::Deferred,
        },
        Edge {
            id: "G04",
            from: GapStatus::Specified,
            event: GapEvent::Resolve,
            guard: None,
            to: GapStatus::Resolved,
        },
        Edge {
            id: "G05",
            from: GapStatus::Specified,
            event: GapEvent::Defer,
            guard: None,
            to: GapStatus::Deferred,
        },
        Edge {
            id: "G06",
            from: GapStatus::Resolved,
            event: GapEvent::Reopen,
            guard: None,
            to: GapStatus::Open,
        },
        Edge {
            id: "G07",
            from: GapStatus::Deferred,
            event: GapEvent::Reopen,
            guard: None,
            to: GapStatus::Open,
        },
    ],
};

impl GapStatus {
    /// The one transition function of `GapViewMachine`.
    pub fn apply(self, event: GapEvent) -> Result<Self, TransitionRefused> {
        transition(&GAP_VIEW_MACHINE, self, event, |_| false).map(|edge| edge.to)
    }
}

impl StateMachineApply for GapStatus {
    type Event = GapEvent;
    type Facts = ();
    fn apply(&self, event: GapEvent, _facts: &()) -> Result<Self, TransitionRefused> {
        GapStatus::apply(*self, event)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_types::transition_refused::TransitionRefusalReason;

    // spec: MEDULLA-ORCH-VOCAB-R815.4
    #[test]
    fn every_legal_transition_applies() {
        let machine = &GAP_VIEW_MACHINE;
        assert_eq!(machine.edges.len(), 7);
        for edge in machine.edges {
            assert_eq!(edge.from.apply(edge.event), Ok(edge.to), "{}", edge.id);
        }
    }

    // spec: MEDULLA-ORCH-VOCAB-R815.4
    #[test]
    fn every_illegal_pair_is_refused_with_transition_refused() {
        let machine = &GAP_VIEW_MACHINE;
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
                assert_eq!(refused.reason, TransitionRefusalReason::IllegalTransition);
                assert_eq!(refused.machine, "GapViewMachine");
                assert_eq!(refused.from, state.stored());
                assert_eq!(refused.event, event.name());
                assert_eq!(refused.code().as_str(), "TRANSITION_REFUSED");
            }
        }
        assert_eq!(illegal, 4 * 4 - 7);
    }

    /// Every production assignment of the status field stores the result of `apply`.
    // spec: MEDULLA-ORCH-VOCAB-R815.4
    #[test]
    fn no_writer_sets_status_outside_apply() {
        for source in [include_str!("gap.rs"), include_str!("lifecycle.rs")] {
            let production = source.split("#[cfg(test)]").next().unwrap();
            let writes: Vec<&str> = production
                .lines()
                .map(str::trim)
                .filter(|l| !l.starts_with("//") && l.contains(".status = "))
                .collect();
            assert!(
                writes.iter().all(|line| *line == "self.status = next;"),
                "{writes:?}"
            );
        }
        let machine = include_str!("gap_machine.rs");
        let production = machine.split("#[cfg(test)]").next().unwrap();
        assert!(!production.contains(".status = "));
        assert!(!production.contains("LEGAL_TRANSITIONS"));
    }
}
