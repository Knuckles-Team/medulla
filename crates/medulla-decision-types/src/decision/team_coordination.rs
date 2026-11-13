//! Team coordination: the channel signal and the team task machine
//! (MEDULLA-DECISION-ENGINE-R170.1).
//!
//! A [`ChannelSignal`] is what a caller observed about one channel: whether it
//! is available and when it was last seen. `TeamTask` is an A2A task a team
//! works on; its status is the explicit machine [`TEAM_TASK_MACHINE`], whose
//! ontology half is `:TeamTaskMachine` in `crates/medulla-core/ontology/a2a-v1.ttl`.
//! Nothing here reads a clock or a store.

use medulla_types::contract::OpaqueId;
use medulla_types::transition_refused::{
    transition, Edge, MachineDef, MachineEvent, MachineState, StateMachineApply, TransitionRefused,
};
use serde::{Deserialize, Serialize};

/// What a caller observed about one channel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ChannelSignal {
    pub channel_id: OpaqueId,
    pub available: bool,
    /// Unix milliseconds of the observation.
    pub observed_at_ms: u64,
}

/// The machine instance name: the `machine` field of every team task refusal
/// and the local name of the ontology instance `:TeamTaskMachine`.
pub const TEAM_TASK_MACHINE_NAME: &str = "TeamTaskMachine";

/// The status of a team task: the one persisted field of its machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum TeamTaskStatus {
    Submitted,
    Working,
    InputRequired,
    Completed,
    Canceled,
    Failed,
}

impl TeamTaskStatus {
    /// Every status, in declaration order.
    pub const ALL: [Self; 6] = [
        Self::Submitted,
        Self::Working,
        Self::InputRequired,
        Self::Completed,
        Self::Canceled,
        Self::Failed,
    ];

    /// The ontology local name: `:TeamTaskMachine_<this>`.
    pub fn ontology_name(self) -> &'static str {
        match self {
            Self::Submitted => "Submitted",
            Self::Working => "Working",
            Self::InputRequired => "InputRequired",
            Self::Completed => "Completed",
            Self::Canceled => "Canceled",
            Self::Failed => "Failed",
        }
    }
}

impl MachineState for TeamTaskStatus {
    fn stored(self) -> &'static str {
        match self {
            Self::Submitted => "submitted",
            Self::Working => "working",
            Self::InputRequired => "input_required",
            Self::Completed => "completed",
            Self::Canceled => "canceled",
            Self::Failed => "failed",
        }
    }
}

/// Every event of the team task machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TeamTaskEvent {
    /// A team member took the task.
    Accept,
    /// The member needs input from the requester.
    RequestInput,
    /// The requester supplied the input.
    ProvideInput,
    /// The work finished.
    Complete,
    /// The task was cancelled.
    Cancel,
    /// The task failed or was rejected.
    Fail,
}

impl TeamTaskEvent {
    /// Every event, in declaration order.
    pub const ALL: [Self; 6] = [
        Self::Accept,
        Self::RequestInput,
        Self::ProvideInput,
        Self::Complete,
        Self::Cancel,
        Self::Fail,
    ];

    /// The ontology local name: `:TeamTaskMachine_Event_<this>`.
    pub fn ontology_name(self) -> &'static str {
        match self {
            Self::Accept => "Accept",
            Self::RequestInput => "RequestInput",
            Self::ProvideInput => "ProvideInput",
            Self::Complete => "Complete",
            Self::Cancel => "Cancel",
            Self::Fail => "Fail",
        }
    }
}

impl MachineEvent for TeamTaskEvent {
    fn name(self) -> &'static str {
        match self {
            Self::Accept => "accept",
            Self::RequestInput => "request_input",
            Self::ProvideInput => "provide_input",
            Self::Complete => "complete",
            Self::Cancel => "cancel",
            Self::Fail => "fail",
        }
    }
}

/// The team task machine has no guards: every edge is decided by state and event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TeamTaskGuard {}

use TeamTaskEvent as Ev;
use TeamTaskStatus as S;

const fn edge(id: &'static str, from: S, event: Ev, to: S) -> Edge<S, Ev, TeamTaskGuard> {
    Edge {
        id,
        from,
        event,
        guard: None,
        to,
    }
}

/// The edge table T01 to T10.
pub const TEAM_TASK_EDGES: &[Edge<S, Ev, TeamTaskGuard>] = &[
    edge("T01", S::Submitted, Ev::Accept, S::Working),
    edge("T02", S::Submitted, Ev::Cancel, S::Canceled),
    edge("T03", S::Submitted, Ev::Fail, S::Failed),
    edge("T04", S::Working, Ev::RequestInput, S::InputRequired),
    edge("T05", S::Working, Ev::Complete, S::Completed),
    edge("T06", S::Working, Ev::Cancel, S::Canceled),
    edge("T07", S::Working, Ev::Fail, S::Failed),
    edge("T08", S::InputRequired, Ev::ProvideInput, S::Working),
    edge("T09", S::InputRequired, Ev::Cancel, S::Canceled),
    edge("T10", S::InputRequired, Ev::Fail, S::Failed),
];

/// The authoritative definition of the team task machine.
pub const TEAM_TASK_MACHINE: MachineDef<S, Ev, TeamTaskGuard> = MachineDef {
    name: TEAM_TASK_MACHINE_NAME,
    states: &TeamTaskStatus::ALL,
    initial: S::Submitted,
    terminals: &[S::Completed, S::Canceled, S::Failed],
    events: &TeamTaskEvent::ALL,
    guards: &[],
    edges: TEAM_TASK_EDGES,
};

impl TeamTaskStatus {
    /// The one transition function: the next status, or the typed refusal.
    pub fn apply(self, event: TeamTaskEvent) -> Result<Self, TransitionRefused> {
        transition(&TEAM_TASK_MACHINE, self, event, |guard| match guard {}).map(|edge| edge.to)
    }
}

impl StateMachineApply for TeamTaskStatus {
    type Event = TeamTaskEvent;
    type Facts = ();

    fn apply(&self, event: TeamTaskEvent, _facts: &()) -> Result<Self, TransitionRefused> {
        TeamTaskStatus::apply(*self, event)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_types::transition_refused::TransitionRefusalReason;

    // spec: MEDULLA-DECISION-ENGINE-R170.1
    #[test]
    fn channel_signal_round_trip() {
        let signal = ChannelSignal {
            channel_id: OpaqueId::new("channel:slack-ops").unwrap(),
            available: false,
            observed_at_ms: 1_700_000_000_123,
        };
        let wire = serde_json::to_value(&signal).unwrap();
        assert_eq!(wire["available"], false);
        assert_eq!(
            serde_json::from_value::<ChannelSignal>(wire.clone()).unwrap(),
            signal
        );
        let packed = rmp_serde::to_vec_named(&signal).unwrap();
        assert_eq!(
            rmp_serde::from_slice::<ChannelSignal>(&packed).unwrap(),
            signal
        );
        let mut unknown = wire;
        unknown["extra"] = serde_json::json!(1);
        assert!(serde_json::from_value::<ChannelSignal>(unknown).is_err());
        assert_eq!(
            serde_json::to_string(&crate::decision::statistical::QuestionKind::TeamCoordination)
                .unwrap(),
            "\"team_coordination\""
        );
    }

    // spec: MEDULLA-DECISION-ENGINE-R170.1
    #[test]
    fn team_task_machine_accepts_every_legal_edge_and_refuses_every_other() {
        for status in TeamTaskStatus::ALL {
            for event in TeamTaskEvent::ALL {
                let legal = TEAM_TASK_EDGES
                    .iter()
                    .find(|edge| edge.from == status && edge.event == event);
                match (legal, status.apply(event)) {
                    (Some(edge), Ok(next)) => assert_eq!(next, edge.to),
                    (None, Err(refused)) => {
                        assert_eq!(refused.machine, TEAM_TASK_MACHINE_NAME);
                        assert_eq!(refused.from, status.stored());
                        assert_eq!(refused.event, event.name());
                        let terminal = TEAM_TASK_MACHINE.is_terminal(status);
                        assert_eq!(
                            refused.reason,
                            if terminal {
                                TransitionRefusalReason::TerminalState
                            } else {
                                TransitionRefusalReason::IllegalTransition
                            }
                        );
                    }
                    (legal, outcome) => panic!("{status:?} {event:?}: {legal:?} vs {outcome:?}"),
                }
            }
        }
        // The six wire forms, and the stored text round trip.
        for status in TeamTaskStatus::ALL {
            let wire = serde_json::to_value(status).unwrap();
            assert_eq!(wire, status.stored());
            assert_eq!(
                TEAM_TASK_MACHINE.state_from_stored(status.stored()),
                Some(status)
            );
        }
        assert_eq!(TEAM_TASK_EDGES.len(), 10);
        // The happy path and a cancel from input_required.
        let working = S::Submitted.apply(Ev::Accept).unwrap();
        assert_eq!(working.apply(Ev::Complete).unwrap(), S::Completed);
        let waiting = working.apply(Ev::RequestInput).unwrap();
        assert_eq!(waiting.apply(Ev::Cancel).unwrap(), S::Canceled);
    }
}
