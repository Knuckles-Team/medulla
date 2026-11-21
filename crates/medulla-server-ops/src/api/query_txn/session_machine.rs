//! MEDULLA-UNIFIED-DATA-PLANE-R768.7: the lifecycle of one bounded foreign-read execution as
//! an explicit state machine (organization constitution Article XXXI).
//!
//! `Open` is the initial state. `Complete` and `Abort` each leave it for a terminal state, so a
//! finished execution accepts no further event. [`SessionState::apply`] is the only function
//! that produces a new state: the four writers in `foreign_read.rs` assign its result, and a
//! refused event is the typed `TRANSITION_REFUSED`. The ontology description is
//! `SessionMachine` in `state-machines-v1.ttl`.

/// The state of one foreign-read execution: the closed state set of `SessionMachine`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionState {
    /// Rows are still being appended.
    Open,
    /// The host finished the execution; its rows are readable.
    Complete,
    /// The host abandoned the execution.
    Aborted,
}

use medulla_types::transition_refused::{
    transition, Edge, MachineDef, MachineEvent, MachineState, TransitionRefused,
};

impl MachineState for SessionState {
    fn stored(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Complete => "complete",
            Self::Aborted => "aborted",
        }
    }
}

/// The closed event set of `SessionMachine`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SessionEvent {
    /// The `Complete` event.
    Complete,
    /// The `Abort` event.
    Abort,
}

impl MachineEvent for SessionEvent {
    fn name(self) -> &'static str {
        match self {
            Self::Complete => "Complete",
            Self::Abort => "Abort",
        }
    }
}

/// `SessionMachine`: the closed edge table. Every other pair of state and event is refused.
pub static SESSION_MACHINE: MachineDef<SessionState, SessionEvent, &'static str> = MachineDef {
    name: "SessionMachine",
    states: &[
        SessionState::Open,
        SessionState::Complete,
        SessionState::Aborted,
    ],
    initial: SessionState::Open,
    terminals: &[SessionState::Complete, SessionState::Aborted],
    events: &[SessionEvent::Complete, SessionEvent::Abort],
    guards: &[],
    edges: &[
        Edge {
            id: "S01",
            from: SessionState::Open,
            event: SessionEvent::Complete,
            guard: None,
            to: SessionState::Complete,
        },
        Edge {
            id: "S02",
            from: SessionState::Open,
            event: SessionEvent::Abort,
            guard: None,
            to: SessionState::Aborted,
        },
    ],
};

impl SessionState {
    /// The one transition function of `SessionMachine`.
    pub fn apply(self, event: SessionEvent) -> Result<Self, TransitionRefused> {
        transition(&SESSION_MACHINE, self, event, |_| false).map(|edge| edge.to)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_types::transition_refused::TransitionRefusalReason;

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R768.7
    #[test]
    fn every_legal_transition_applies() {
        assert_eq!(SESSION_MACHINE.edges.len(), 2);
        for edge in SESSION_MACHINE.edges {
            assert_eq!(edge.from.apply(edge.event), Ok(edge.to), "edge {}", edge.id);
        }
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R768.7
    #[test]
    fn every_illegal_pair_is_refused_with_transition_refused() {
        let machine = &SESSION_MACHINE;
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
                assert_eq!(refused.machine, "SessionMachine");
                assert_eq!(refused.from, state.stored());
                assert_eq!(refused.event, event.name());
                assert_eq!(refused.reason, TransitionRefusalReason::TerminalState);
                assert_eq!(refused.code().as_str(), "TRANSITION_REFUSED");
                refused_pairs += 1;
            }
        }
        assert_eq!(refused_pairs, 4);
    }

    /// Lines of `foreign_read.rs` that assign a `state` field.
    // spec: MEDULLA-UNIFIED-DATA-PLANE-R768.7
    #[test]
    fn no_writer_sets_state_outside_apply() {
        let source = include_str!("foreign_read.rs");
        let production = source.split("#[cfg(test)]").next().unwrap();
        let writes: Vec<&str> = production
            .lines()
            .map(str::trim)
            .filter(|line| !line.starts_with("//") && line.contains(".state = "))
            .collect();
        assert_eq!(writes.len(), 2);
        assert!(writes
            .iter()
            .all(|line| line.contains("record.state.apply(")));
    }
}
