//! MEDULLA-SDLC-R030.5 -- the task state as one explicit state machine.
//!
//! `TaskState::apply` is the one transition function. No state is terminal: a task's state is derived from
//! its checkbox and its unresolved targets.

use medulla_types::transition_refused::{
    transition, Edge, MachineDef, MachineEvent, MachineState, StateMachineApply, TransitionRefused,
};

use crate::lifecycle::TaskState;

/// An event of `TaskMachine`: the verb of the code path that moves the state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TaskEvent {
    /// An unresolved target blocks the task.
    Block,
    /// Every target resolves.
    Unblock,
    /// The checkbox is ticked.
    Complete,
    /// The checkbox is cleared.
    Reopen,
}

impl MachineState for TaskState {
    fn stored(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Blocked => "blocked",
            Self::Done => "done",
        }
    }
}

impl MachineEvent for TaskEvent {
    fn name(self) -> &'static str {
        match self {
            Self::Block => "Block",
            Self::Unblock => "Unblock",
            Self::Complete => "Complete",
            Self::Reopen => "Reopen",
        }
    }
}

/// `TaskMachine`: the closed edge table. Every other pair of state and event is refused.
pub static TASK_MACHINE: MachineDef<TaskState, TaskEvent, &'static str> = MachineDef {
    name: "TaskMachine",
    states: &[TaskState::Open, TaskState::Blocked, TaskState::Done],
    initial: TaskState::Open,
    terminals: &[],
    events: &[
        TaskEvent::Block,
        TaskEvent::Unblock,
        TaskEvent::Complete,
        TaskEvent::Reopen,
    ],
    guards: &[],
    edges: &[
        Edge {
            id: "T01",
            from: TaskState::Open,
            event: TaskEvent::Block,
            guard: None,
            to: TaskState::Blocked,
        },
        Edge {
            id: "T02",
            from: TaskState::Blocked,
            event: TaskEvent::Unblock,
            guard: None,
            to: TaskState::Open,
        },
        Edge {
            id: "T03",
            from: TaskState::Open,
            event: TaskEvent::Complete,
            guard: None,
            to: TaskState::Done,
        },
        Edge {
            id: "T04",
            from: TaskState::Blocked,
            event: TaskEvent::Complete,
            guard: None,
            to: TaskState::Done,
        },
        Edge {
            id: "T05",
            from: TaskState::Done,
            event: TaskEvent::Reopen,
            guard: None,
            to: TaskState::Open,
        },
    ],
};

impl TaskState {
    /// The state a new record starts in.
    pub fn initial() -> Self {
        TASK_MACHINE.initial
    }

    /// The one transition function of `TaskMachine`.
    pub fn apply(self, event: TaskEvent) -> Result<Self, TransitionRefused> {
        transition(&TASK_MACHINE, self, event, |_| false).map(|edge| edge.to)
    }
}

impl StateMachineApply for TaskState {
    type Event = TaskEvent;
    type Facts = ();
    fn apply(&self, event: TaskEvent, _facts: &()) -> Result<Self, TransitionRefused> {
        TaskState::apply(*self, event)
    }
}

impl TaskState {
    /// The state a task's checkbox and unresolved targets put it in, reached by applying
    /// events to the initial state.
    pub fn derive(ticked: bool, blocked: bool) -> Self {
        let state = Self::initial();
        if ticked {
            return state
                .apply(TaskEvent::Complete)
                .expect("an open task accepts Complete");
        }
        if blocked {
            return state
                .apply(TaskEvent::Block)
                .expect("an open task accepts Block");
        }
        state
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_types::transition_refused::TransitionRefusalReason;

    // spec: MEDULLA-SDLC-R030.5
    #[test]
    fn derive_walks_the_edge_table() {
        assert_eq!(TaskState::derive(true, true), TaskState::Done);
        assert_eq!(TaskState::derive(false, true), TaskState::Blocked);
        assert_eq!(TaskState::derive(false, false), TaskState::Open);
    }

    // spec: MEDULLA-SDLC-R030.5
    #[test]
    fn every_legal_transition_applies() {
        let machine = &TASK_MACHINE;
        assert_eq!(machine.edges.len(), 5);
        for edge in machine.edges {
            assert_eq!(edge.from.apply(edge.event), Ok(edge.to), "{}", edge.id);
        }
        let mut state = TaskState::initial();
        for event in [TaskEvent::Block, TaskEvent::Complete] {
            state = state.apply(event).unwrap();
        }
        assert_eq!(state, TaskState::Done);
    }

    // spec: MEDULLA-SDLC-R030.5
    #[test]
    fn every_illegal_pair_is_refused_with_transition_refused() {
        let machine = &TASK_MACHINE;
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
                assert_eq!(refused.machine, "TaskMachine");
                assert_eq!(refused.from, state.stored());
                assert_eq!(refused.event, event.name());
                assert_eq!(refused.code().as_str(), "TRANSITION_REFUSED");
            }
        }
        assert_eq!(illegal, 3 * 4 - 5);
    }

    /// Production lines that assign the state outside `apply`: every record is built
    /// from `TaskState::initial()` or an `apply` result, never from a bare variant.
    // spec: MEDULLA-SDLC-R030.5
    #[test]
    fn no_writer_sets_state_outside_apply() {
        for source in [include_str!("../../medulla-core/src/lifecycle/status.rs")] {
            let production = source.split("#[cfg(test)]").next().unwrap();
            let writes: Vec<&str> = production
                .lines()
                .map(str::trim)
                .filter(|l| !l.starts_with("//"))
                .filter(|l| {
                    TASK_MACHINE.states.iter().any(|s| {
                        let variant = format!("TaskState::{s:?}");
                        l.contains(&format!("state: {variant}"))
                            || l.contains(&format!(".state = {variant}"))
                    })
                })
                .collect();
            assert!(writes.is_empty(), "{writes:?}");
        }
    }
}
