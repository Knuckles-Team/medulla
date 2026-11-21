//! MEDULLA-ORCH-VOCAB-R815.2 -- the agent instance lifecycle as one explicit edge table
//! (organization constitution Article XXXI).
//!
//! The lifecycle moves through `Method::Statechart` on the chart
//! [`super::org::AGENT_INSTANCE_CHART`]; this table is the one source the chart is built from
//! (`agent_instance_statechart_def`), and [`AgentLifecycleState::apply`] is the typed
//! transition function over it.

use medulla_types::transition_refused::{
    transition, Edge, MachineDef, MachineEvent, MachineState, StateMachineApply, TransitionRefused,
};

use super::org::AgentLifecycleState;

/// An event of `AgentLifecycleMachine`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AgentLifecycleEvent {
    /// The registered instance starts work.
    Activate,
    /// The active instance stops taking new work.
    Drain,
    /// The instance ends.
    Terminate,
}

impl MachineState for AgentLifecycleState {
    fn stored(self) -> &'static str {
        self.as_str()
    }
}

impl MachineEvent for AgentLifecycleEvent {
    fn name(self) -> &'static str {
        match self {
            Self::Activate => "activate",
            Self::Drain => "drain",
            Self::Terminate => "terminate",
        }
    }
}

/// `AgentLifecycleMachine`: the closed edge table. Every other pair of state and event is
/// refused.
pub static AGENT_LIFECYCLE_MACHINE: MachineDef<
    AgentLifecycleState,
    AgentLifecycleEvent,
    &'static str,
> = MachineDef {
    name: "AgentLifecycleMachine",
    states: &[
        AgentLifecycleState::Registered,
        AgentLifecycleState::Active,
        AgentLifecycleState::Draining,
        AgentLifecycleState::Terminated,
    ],
    initial: AgentLifecycleState::Registered,
    terminals: &[AgentLifecycleState::Terminated],
    events: &[
        AgentLifecycleEvent::Activate,
        AgentLifecycleEvent::Drain,
        AgentLifecycleEvent::Terminate,
    ],
    guards: &[],
    edges: &[
        Edge {
            id: "A01",
            from: AgentLifecycleState::Registered,
            event: AgentLifecycleEvent::Activate,
            guard: None,
            to: AgentLifecycleState::Active,
        },
        Edge {
            id: "A02",
            from: AgentLifecycleState::Registered,
            event: AgentLifecycleEvent::Terminate,
            guard: None,
            to: AgentLifecycleState::Terminated,
        },
        Edge {
            id: "A03",
            from: AgentLifecycleState::Active,
            event: AgentLifecycleEvent::Drain,
            guard: None,
            to: AgentLifecycleState::Draining,
        },
        Edge {
            id: "A04",
            from: AgentLifecycleState::Draining,
            event: AgentLifecycleEvent::Terminate,
            guard: None,
            to: AgentLifecycleState::Terminated,
        },
    ],
};

impl AgentLifecycleState {
    /// The one transition function of `AgentLifecycleMachine`.
    pub fn apply(self, event: AgentLifecycleEvent) -> Result<Self, TransitionRefused> {
        transition(&AGENT_LIFECYCLE_MACHINE, self, event, |_| false).map(|edge| edge.to)
    }
}

impl StateMachineApply for AgentLifecycleState {
    type Event = AgentLifecycleEvent;
    type Facts = ();
    fn apply(&self, event: AgentLifecycleEvent, _facts: &()) -> Result<Self, TransitionRefused> {
        AgentLifecycleState::apply(*self, event)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_types::transition_refused::TransitionRefusalReason;

    // spec: MEDULLA-ORCH-VOCAB-R815.2
    #[test]
    fn every_legal_transition_applies() {
        let machine = &AGENT_LIFECYCLE_MACHINE;
        assert_eq!(machine.edges.len(), 4);
        for edge in machine.edges {
            assert_eq!(edge.from.apply(edge.event), Ok(edge.to), "{}", edge.id);
        }
    }

    // spec: MEDULLA-ORCH-VOCAB-R815.2
    #[test]
    fn every_illegal_pair_is_refused_with_transition_refused() {
        let machine = &AGENT_LIFECYCLE_MACHINE;
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
                let reason = if machine.is_terminal(state) {
                    TransitionRefusalReason::TerminalState
                } else {
                    TransitionRefusalReason::IllegalTransition
                };
                assert_eq!(refused.reason, reason, "{state:?} {event:?}");
                assert_eq!(refused.machine, "AgentLifecycleMachine");
                assert_eq!(refused.from, state.stored());
                assert_eq!(refused.event, event.name());
                assert_eq!(refused.code().as_str(), "TRANSITION_REFUSED");
            }
        }
        assert_eq!(illegal, 4 * 3 - 4);
    }
}
