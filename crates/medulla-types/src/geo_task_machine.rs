//! MEDULLA-FEDERATED-QUERY-R738.8: the lifecycle status of a geolocated task as one explicit state machine
//! (organization constitution Article XXXI).
//!
//! The machine runs on the `Copy` discriminant [`TaskStatusKind`]; any data a record carries beside its
//! state stays on the record and is set at the transition. [`TaskStatusKind::apply`] is the one
//! transition function, and an illegal pair is the typed `TRANSITION_REFUSED`. The ontology
//! description is `GeoTaskMachine` in `state-machines-v1.ttl`.

use serde::{Deserialize, Serialize};

use crate::transition_refused::{
    transition, Edge, MachineDef, MachineEvent, MachineState, TransitionRefused,
};

/// The closed state set of `GeoTaskMachine`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatusKind {
    /// `pending`.
    Pending,
    /// `assigned`.
    Assigned,
    /// `in_progress`.
    InProgress,
    /// `done`.
    Done,
    /// `cancelled`.
    Cancelled,
}

/// The closed event set of `GeoTaskMachine`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TaskEvent {
    /// A resource was assigned.
    Assign,
    /// Work started.
    Start,
    /// Work finished.
    Complete,
    /// The task was withdrawn.
    Cancel,
}

impl MachineState for TaskStatusKind {
    fn stored(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Assigned => "assigned",
            Self::InProgress => "in_progress",
            Self::Done => "done",
            Self::Cancelled => "cancelled",
        }
    }
}

impl MachineEvent for TaskEvent {
    fn name(self) -> &'static str {
        match self {
            Self::Assign => "Assign",
            Self::Start => "Start",
            Self::Complete => "Complete",
            Self::Cancel => "Cancel",
        }
    }
}

/// `GeoTaskMachine`: the closed edge table. Every other pair is refused.
pub static GEO_TASK_MACHINE: MachineDef<TaskStatusKind, TaskEvent, &'static str> = MachineDef {
    name: "GeoTaskMachine",
    states: &[TaskStatusKind::Pending, TaskStatusKind::Assigned, TaskStatusKind::InProgress, TaskStatusKind::Done, TaskStatusKind::Cancelled],
    initial: TaskStatusKind::Pending,
    terminals: &[TaskStatusKind::Done, TaskStatusKind::Cancelled],
    events: &[TaskEvent::Assign, TaskEvent::Start, TaskEvent::Complete, TaskEvent::Cancel],
    guards: &[],
    edges: &[
        Edge {
            id: "GK01",
            from: TaskStatusKind::Pending,
            event: TaskEvent::Assign,
            guard: None,
            to: TaskStatusKind::Assigned,
        },
        Edge {
            id: "GK02",
            from: TaskStatusKind::Assigned,
            event: TaskEvent::Start,
            guard: None,
            to: TaskStatusKind::InProgress,
        },
        Edge {
            id: "GK03",
            from: TaskStatusKind::InProgress,
            event: TaskEvent::Complete,
            guard: None,
            to: TaskStatusKind::Done,
        },
        Edge {
            id: "GK04",
            from: TaskStatusKind::Pending,
            event: TaskEvent::Cancel,
            guard: None,
            to: TaskStatusKind::Cancelled,
        },
        Edge {
            id: "GK05",
            from: TaskStatusKind::Assigned,
            event: TaskEvent::Cancel,
            guard: None,
            to: TaskStatusKind::Cancelled,
        },
        Edge {
            id: "GK06",
            from: TaskStatusKind::InProgress,
            event: TaskEvent::Cancel,
            guard: None,
            to: TaskStatusKind::Cancelled,
        },
    ],
};

impl TaskStatusKind {
    /// Every state, in declaration order.
    pub const ALL: [TaskStatusKind; 5] = [TaskStatusKind::Pending, TaskStatusKind::Assigned, TaskStatusKind::InProgress, TaskStatusKind::Done, TaskStatusKind::Cancelled];

    /// The state a new record takes.
    pub const fn initial() -> Self {
        TaskStatusKind::Pending
    }

    /// The one transition function of `GeoTaskMachine`.
    pub fn apply(self, event: TaskEvent) -> Result<Self, TransitionRefused> {
        transition(&GEO_TASK_MACHINE, self, event, |_| false).map(|edge| edge.to)
    }

    /// The event of the edge `self -> to`, if the table has one.
    pub fn event_to(self, to: Self) -> Option<TaskEvent> {
        GEO_TASK_MACHINE
            .edges
            .iter()
            .find(|edge| edge.from == self && edge.to == to)
            .map(|edge| edge.event)
    }

    /// Move to `to` along its edge, or refuse with `TRANSITION_REFUSED`.
    pub fn move_to(self, to: Self) -> Result<Self, TransitionRefused> {
        match self.event_to(to) {
            Some(event) => self.apply(event),
            None => Err(TransitionRefused {
                machine: GEO_TASK_MACHINE.name.to_string(),
                from: self.stored().to_string(),
                event: format!("to:{}", to.stored()),
                reason: crate::transition_refused::TransitionRefusalReason::IllegalTransition,
            }),
        }
    }

    /// The state whose stored text is `text`.
    pub fn from_stored(text: &str) -> Option<Self> {
        GEO_TASK_MACHINE.state_from_stored(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transition_refused::TransitionRefusalReason;

    // proves R738.8: GeoTask status is one explicit state machine, and every writer routes through ...
    // spec: MEDULLA-FEDERATED-QUERY-R738.8
    #[test]
    fn every_legal_transition_applies() {
        assert_eq!(GEO_TASK_MACHINE.edges.len(), 6);
        for edge in GEO_TASK_MACHINE.edges {
            assert_eq!(edge.from.apply(edge.event), Ok(edge.to), "{}", edge.id);
            assert_eq!(edge.from.event_to(edge.to), Some(edge.event));
        }
        assert_eq!(TaskStatusKind::initial(), GEO_TASK_MACHINE.initial);
    }

    // spec: MEDULLA-FEDERATED-QUERY-R738.8
    #[test]
    fn every_illegal_pair_is_refused_with_transition_refused() {
        let machine = &GEO_TASK_MACHINE;
        for &state in machine.states {
            for &event in machine.events {
                if machine.edges.iter().any(|e| e.from == state && e.event == event) {
                    continue;
                }
                let refused = state.apply(event).unwrap_err();
                assert_eq!(refused.machine, "GeoTaskMachine");
                assert_eq!(refused.from, state.stored());
                assert_eq!(refused.event, event.name());
                let expected = if machine.terminals.contains(&state) {
                    TransitionRefusalReason::TerminalState
                } else {
                    TransitionRefusalReason::IllegalTransition
                };
                assert_eq!(refused.reason, expected);
            }
        }
        for state in TaskStatusKind::ALL {
            assert_eq!(TaskStatusKind::from_stored(state.stored()), Some(state));
        }
        assert_eq!(TaskStatusKind::from_stored("weather"), None);
    }

    // spec: MEDULLA-FEDERATED-QUERY-R738.8
    #[test]
    fn no_writer_sets_status_outside_apply() {
        let source = include_str!("../../medulla-geo/src/geotask.rs");
        assert!(source.contains("pub fn apply(&mut self, event: TaskEvent)"), "the writer routes through the machine");
        assert!(!source.contains("fn with_status"), "no builder sets the status directly");
        assert!(!source.contains("enum TaskStatus"), "the status enum is the machine kind");
    }
}
