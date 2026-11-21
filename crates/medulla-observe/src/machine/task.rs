//! `TaskMachine` (MEDULLA-OBSERVE-R004.1): the lifecycle of one registered background job.
//!
//! States: Registered (initial), Running, Progressing, Stalled, Completed, Failed and
//! Cancelled; the last three are terminal. [`TaskState::apply`] is the one transition
//! function. Every pair of state and event that no edge lists is refused with
//! `TRANSITION_REFUSED`. `Failed` carries a typed [`TaskFailure`] in the task's record.
//! The edges T01 to T17 are in `specs/engine-observability/data-model.md`.

use medulla_types::transition_refused::{
    transition, Edge, MachineDef, MachineEvent, MachineState, TransitionRefused,
};
use serde::{Deserialize, Serialize};

/// The stored code of a task whose handle was dropped without `finish`.
pub const TASK_DROPPED: &str = "TASK_DROPPED";
/// The stored code of a task whose future panicked.
pub const TASK_PANICKED: &str = "TASK_PANICKED";
/// The code of a task whose guarded loop made no progress.
pub const NO_PROGRESS: &str = "NO_PROGRESS";

/// The closed state set of `TaskMachine`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    /// The job is registered and has not started.
    Registered,
    /// The job runs and has reported no progress yet.
    Running,
    /// The job reported progress.
    Progressing,
    /// The watchdog saw no progress for the stall window.
    Stalled,
    /// The job finished.
    Completed,
    /// The job failed with a typed [`TaskFailure`].
    Failed,
    /// The job was cancelled.
    Cancelled,
}

/// The closed event set of `TaskMachine`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TaskEvent {
    /// The job began to run.
    Start,
    /// The job reported progress.
    Progress,
    /// The watchdog saw no progress for the stall window.
    StallDetected,
    /// The job finished.
    Finish,
    /// The job failed.
    Fail,
    /// The job was cancelled.
    Cancel,
}

/// The guards of `TaskMachine`: none.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskGuard {}

impl MachineState for TaskState {
    fn stored(self) -> &'static str {
        match self {
            Self::Registered => "registered",
            Self::Running => "running",
            Self::Progressing => "progressing",
            Self::Stalled => "stalled",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }
}

impl MachineEvent for TaskEvent {
    fn name(self) -> &'static str {
        match self {
            Self::Start => "Start",
            Self::Progress => "Progress",
            Self::StallDetected => "StallDetected",
            Self::Finish => "Finish",
            Self::Fail => "Fail",
            Self::Cancel => "Cancel",
        }
    }
}

const fn edge(
    id: &'static str,
    from: TaskState,
    event: TaskEvent,
    to: TaskState,
) -> Edge<TaskState, TaskEvent, TaskGuard> {
    Edge {
        id,
        from,
        event,
        guard: None,
        to,
    }
}

use TaskEvent as E;
use TaskState as S;

/// `TaskMachine`: the closed edge table T01 to T17. Every other pair is refused.
pub static TASK_MACHINE: MachineDef<TaskState, TaskEvent, TaskGuard> = MachineDef {
    name: "TaskMachine",
    states: &[
        S::Registered,
        S::Running,
        S::Progressing,
        S::Stalled,
        S::Completed,
        S::Failed,
        S::Cancelled,
    ],
    initial: S::Registered,
    terminals: &[S::Completed, S::Failed, S::Cancelled],
    events: &[
        E::Start,
        E::Progress,
        E::StallDetected,
        E::Finish,
        E::Fail,
        E::Cancel,
    ],
    guards: &[],
    edges: &[
        edge("T01", S::Registered, E::Start, S::Running),
        edge("T02", S::Running, E::Progress, S::Progressing),
        edge("T03", S::Progressing, E::Progress, S::Progressing),
        edge("T04", S::Running, E::StallDetected, S::Stalled),
        edge("T05", S::Progressing, E::StallDetected, S::Stalled),
        edge("T06", S::Stalled, E::Progress, S::Progressing),
        edge("T07", S::Running, E::Finish, S::Completed),
        edge("T08", S::Progressing, E::Finish, S::Completed),
        edge("T09", S::Stalled, E::Finish, S::Completed),
        edge("T10", S::Registered, E::Fail, S::Failed),
        edge("T11", S::Running, E::Fail, S::Failed),
        edge("T12", S::Progressing, E::Fail, S::Failed),
        edge("T13", S::Stalled, E::Fail, S::Failed),
        edge("T14", S::Registered, E::Cancel, S::Cancelled),
        edge("T15", S::Running, E::Cancel, S::Cancelled),
        edge("T16", S::Progressing, E::Cancel, S::Cancelled),
        edge("T17", S::Stalled, E::Cancel, S::Cancelled),
    ],
};

impl TaskState {
    /// Every state, in declaration order.
    pub const ALL: [TaskState; 7] = [
        S::Registered,
        S::Running,
        S::Progressing,
        S::Stalled,
        S::Completed,
        S::Failed,
        S::Cancelled,
    ];

    /// The state a new task takes.
    pub const fn initial() -> Self {
        S::Registered
    }

    /// Whether the state accepts no further event.
    pub fn is_terminal(self) -> bool {
        TASK_MACHINE.is_terminal(self)
    }

    /// The one transition function of `TaskMachine`: the next state, or `TRANSITION_REFUSED`.
    pub fn apply(self, event: TaskEvent) -> Result<Self, TransitionRefused> {
        transition(&TASK_MACHINE, self, event, |guard| match guard {}).map(|edge| edge.to)
    }
}

/// Why a task is `Failed`. The stored code is the one the introspection rows carry.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskFailure {
    /// The handle was dropped without `finish` (`TASK_DROPPED`).
    Dropped,
    /// The job's future panicked at `location` (`TASK_PANICKED`).
    Panicked {
        /// The `file:line` the panic came from.
        location: String,
    },
    /// A guarded loop of the job made no progress (`NO_PROGRESS`).
    NoProgress,
    /// The job returned this typed refusal code.
    Refused {
        /// The refusal code.
        code: String,
    },
}

impl TaskFailure {
    /// The stored code of this failure.
    pub fn code(&self) -> &str {
        match self {
            Self::Dropped => TASK_DROPPED,
            Self::Panicked { .. } => TASK_PANICKED,
            Self::NoProgress => NO_PROGRESS,
            Self::Refused { code } => code,
        }
    }
}
