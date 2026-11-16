//! MEDULLA-INGEST-WORKER-R911.4 -- the trust state of a measured throughput window as one
//! explicit state machine (organization constitution Article XXXI).
//!
//! A window is `Unknown` until fresh telemetry covers its tail, and `Complete` while that
//! telemetry stays fresh. [`ThroughputState::apply`] is the one transition function.

use crate::ingest_throughput::ThroughputState;
use crate::transition_refused::{
    transition, Edge, MachineDef, MachineEvent, MachineState, TransitionRefused,
};

/// An event of `ThroughputMachine`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ThroughputEvent {
    /// Telemetry newer than the freshness floor covers the window tail.
    TelemetryFresh,
    /// The newest telemetry fell behind the freshness floor.
    TelemetryStale,
}

impl MachineState for ThroughputState {
    fn stored(self) -> &'static str {
        match self {
            Self::Complete => "complete",
            Self::Unknown => "unknown",
        }
    }
}

impl MachineEvent for ThroughputEvent {
    fn name(self) -> &'static str {
        match self {
            Self::TelemetryFresh => "TelemetryFresh",
            Self::TelemetryStale => "TelemetryStale",
        }
    }
}

/// `ThroughputMachine`: the closed edge table. Every other pair of state and event is refused.
pub static THROUGHPUT_MACHINE: MachineDef<ThroughputState, ThroughputEvent, &'static str> =
    MachineDef {
        name: "ThroughputMachine",
        states: &[ThroughputState::Complete, ThroughputState::Unknown],
        initial: ThroughputState::Unknown,
        terminals: &[],
        events: &[
            ThroughputEvent::TelemetryFresh,
            ThroughputEvent::TelemetryStale,
        ],
        guards: &[],
        edges: &[
            Edge {
                id: "T01",
                from: ThroughputState::Unknown,
                event: ThroughputEvent::TelemetryFresh,
                guard: None,
                to: ThroughputState::Complete,
            },
            Edge {
                id: "T02",
                from: ThroughputState::Complete,
                event: ThroughputEvent::TelemetryStale,
                guard: None,
                to: ThroughputState::Unknown,
            },
        ],
    };

impl ThroughputState {
    /// The one transition function of `ThroughputMachine`.
    pub fn apply(self, event: ThroughputEvent) -> Result<Self, TransitionRefused> {
        transition(&THROUGHPUT_MACHINE, self, event, |_| false).map(|edge| edge.to)
    }

    /// The state of a new window given whether fresh telemetry covers its tail: the
    /// initial state, advanced by `TelemetryFresh` when it does.
    pub fn observe(fresh: bool) -> Result<Self, TransitionRefused> {
        let initial = THROUGHPUT_MACHINE.initial;
        if fresh {
            initial.apply(ThroughputEvent::TelemetryFresh)
        } else {
            Ok(initial)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transition_refused::TransitionRefusalReason;

    // spec: MEDULLA-INGEST-WORKER-R911.4
    #[test]
    fn every_legal_transition_applies() {
        for edge in THROUGHPUT_MACHINE.edges {
            assert_eq!(edge.from.apply(edge.event), Ok(edge.to), "{}", edge.id);
        }
        assert_eq!(
            ThroughputState::observe(true),
            Ok(ThroughputState::Complete)
        );
        assert_eq!(
            ThroughputState::observe(false),
            Ok(ThroughputState::Unknown)
        );
    }

    // spec: MEDULLA-INGEST-WORKER-R911.4
    #[test]
    fn every_illegal_pair_is_refused_with_transition_refused() {
        let machine = &THROUGHPUT_MACHINE;
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
                assert_eq!(refused.machine, "ThroughputMachine");
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

    /// Lines that assign the `state` field outside the transition function.
    // spec: MEDULLA-INGEST-WORKER-R911.4
    #[test]
    fn no_writer_sets_state_outside_apply() {
        let source = include_str!("ingest_throughput.rs");
        let production = source.split("#[cfg(test)]").next().unwrap();
        let writes: Vec<&str> = production
            .lines()
            .map(str::trim)
            .filter(|l| !l.starts_with("//") && l.contains(".state = "))
            .collect();
        assert!(writes.is_empty(), "{writes:?}");
        let literals: Vec<&str> = production
            .lines()
            .map(str::trim)
            .filter(|l| l.starts_with("state: ThroughputState::"))
            .collect();
        assert!(literals.is_empty(), "{literals:?}");
    }
}
