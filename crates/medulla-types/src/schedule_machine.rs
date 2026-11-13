//! The schedule pause machine `ScheduleMachine` (MEDULLA-PLACEMENT-R751.756,
//! MEDULLA-PLACEMENT-R785.1, organization constitution Article XXXI).
//!
//! [`ScheduleState`] is the closed state set, [`ScheduleEvent`] the closed event set,
//! [`SCHEDULE_MACHINE`] the edge table and [`ScheduleState::apply`] the one transition
//! function. The machine lives in this leaf types crate so the stored schedule record
//! (`medulla-jobs`) and the served `ScheduleView` (`medulla-protocol-types`) carry the same
//! closed enum.

use serde::{Deserialize, Serialize};

use crate::transition_refused::{
    transition, Edge, MachineDef, MachineEvent, MachineState, TransitionRefused,
};

/// The closed state set of the schedule machine.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum ScheduleState {
    #[default]
    Enabled,
    Paused,
}

impl MachineState for ScheduleState {
    fn stored(self) -> &'static str {
        match self {
            Self::Enabled => "enabled",
            Self::Paused => "paused",
        }
    }
}

/// The closed event set of the schedule machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScheduleEvent {
    Pause,
    Resume,
}

impl MachineEvent for ScheduleEvent {
    fn name(self) -> &'static str {
        match self {
            Self::Pause => "pause",
            Self::Resume => "resume",
        }
    }
}

/// The schedule machine has no guards.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScheduleGuard {}

/// The schedule machine: `Enabled` start, no terminal state. Pausing a paused schedule and
/// resuming an enabled one are illegal transitions; the served methods answer them
/// idempotently before calling the machine.
pub const SCHEDULE_MACHINE: MachineDef<ScheduleState, ScheduleEvent, ScheduleGuard> = MachineDef {
    name: "ScheduleMachine",
    states: &[ScheduleState::Enabled, ScheduleState::Paused],
    initial: ScheduleState::Enabled,
    terminals: &[],
    events: &[ScheduleEvent::Pause, ScheduleEvent::Resume],
    guards: &[],
    edges: &[
        Edge {
            id: "S01",
            from: ScheduleState::Enabled,
            event: ScheduleEvent::Pause,
            guard: None,
            to: ScheduleState::Paused,
        },
        Edge {
            id: "S02",
            from: ScheduleState::Paused,
            event: ScheduleEvent::Resume,
            guard: None,
            to: ScheduleState::Enabled,
        },
    ],
};

impl ScheduleState {
    /// The one transition function: the next state, or the typed refusal.
    pub fn apply(self, event: ScheduleEvent) -> Result<Self, TransitionRefused> {
        transition(&SCHEDULE_MACHINE, self, event, |guard| match guard {}).map(|edge| edge.to)
    }

    /// The state a pause or resume leaves the schedule in.
    pub fn target(event: ScheduleEvent) -> Self {
        match event {
            ScheduleEvent::Pause => Self::Paused,
            ScheduleEvent::Resume => Self::Enabled,
        }
    }
}
