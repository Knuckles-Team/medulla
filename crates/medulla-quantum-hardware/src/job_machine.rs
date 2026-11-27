//! MEDULLA-CONTRACT-R062.2 -- the status of a hardware job as one explicit state machine
//! (organization constitution Article XXXI).
//!
//! `JobStatus` carries data (`Failed(String)`), so the machine runs on the `Copy` kind
//! [`HardwareJobPhase`]. A job starts `Queued`; each provider poll reports a status, and a
//! cancel the provider accepted is its own event. [`HardwareJobPhase::apply`] is the one
//! transition function, and a refused pair is the typed `TRANSITION_REFUSED`. The ontology
//! description is `HardwareJobMachine` in `state-machines-v1.ttl`.

use medulla_quantum_core::backend::JobStatus;
use medulla_types::transition_refused::{
    transition, Edge, MachineDef, MachineEvent, MachineState, TransitionRefused,
};

/// The kind of a job status: the closed state set of `HardwareJobMachine`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HardwareJobPhase {
    /// Submitted and not yet running.
    Queued,
    /// The provider is running the job.
    Running,
    /// The job finished and has a result.
    Completed,
    /// The job failed on the provider side.
    Failed,
    /// The job was cancelled.
    Cancelled,
}

/// An event of `HardwareJobMachine`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HardwareJobEvent {
    /// A poll reported the job queued.
    ProviderReportedQueued,
    /// A poll reported the job running.
    ProviderReportedRunning,
    /// A poll reported the job completed.
    ProviderReportedCompleted,
    /// A poll reported the job failed.
    ProviderReportedFailed,
    /// A poll reported the job cancelled.
    ProviderReportedCancelled,
    /// The provider accepted a cancel request.
    CancelAccepted,
}

impl MachineState for HardwareJobPhase {
    fn stored(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }
}

impl MachineEvent for HardwareJobEvent {
    fn name(self) -> &'static str {
        match self {
            Self::ProviderReportedQueued => "ProviderReportedQueued",
            Self::ProviderReportedRunning => "ProviderReportedRunning",
            Self::ProviderReportedCompleted => "ProviderReportedCompleted",
            Self::ProviderReportedFailed => "ProviderReportedFailed",
            Self::ProviderReportedCancelled => "ProviderReportedCancelled",
            Self::CancelAccepted => "CancelAccepted",
        }
    }
}

impl HardwareJobPhase {
    /// The phase a status is in.
    pub fn of(status: &JobStatus) -> Self {
        match status {
            JobStatus::Queued => Self::Queued,
            JobStatus::Running => Self::Running,
            JobStatus::Completed => Self::Completed,
            JobStatus::Failed(_) => Self::Failed,
            JobStatus::Cancelled => Self::Cancelled,
        }
    }
}

impl HardwareJobEvent {
    /// The event a provider-reported status raises.
    pub fn reported(status: &JobStatus) -> Self {
        match status {
            JobStatus::Queued => Self::ProviderReportedQueued,
            JobStatus::Running => Self::ProviderReportedRunning,
            JobStatus::Completed => Self::ProviderReportedCompleted,
            JobStatus::Failed(_) => Self::ProviderReportedFailed,
            JobStatus::Cancelled => Self::ProviderReportedCancelled,
        }
    }
}

/// `HardwareJobMachine`: the closed edge table. Every other pair of state and event is
/// refused: a running job reported queued is an `illegal_transition`, and any event from a
/// terminal state is a `terminal_state`.
pub static HARDWARE_JOB_MACHINE: MachineDef<HardwareJobPhase, HardwareJobEvent, &'static str> =
    MachineDef {
        name: "HardwareJobMachine",
        states: &[
            HardwareJobPhase::Queued,
            HardwareJobPhase::Running,
            HardwareJobPhase::Completed,
            HardwareJobPhase::Failed,
            HardwareJobPhase::Cancelled,
        ],
        initial: HardwareJobPhase::Queued,
        terminals: &[
            HardwareJobPhase::Completed,
            HardwareJobPhase::Failed,
            HardwareJobPhase::Cancelled,
        ],
        events: &[
            HardwareJobEvent::ProviderReportedQueued,
            HardwareJobEvent::ProviderReportedRunning,
            HardwareJobEvent::ProviderReportedCompleted,
            HardwareJobEvent::ProviderReportedFailed,
            HardwareJobEvent::ProviderReportedCancelled,
            HardwareJobEvent::CancelAccepted,
        ],
        guards: &[],
        edges: &[
            Edge {
                id: "J01",
                from: HardwareJobPhase::Queued,
                event: HardwareJobEvent::ProviderReportedQueued,
                guard: None,
                to: HardwareJobPhase::Queued,
            },
            Edge {
                id: "J02",
                from: HardwareJobPhase::Queued,
                event: HardwareJobEvent::ProviderReportedRunning,
                guard: None,
                to: HardwareJobPhase::Running,
            },
            Edge {
                id: "J03",
                from: HardwareJobPhase::Queued,
                event: HardwareJobEvent::ProviderReportedCompleted,
                guard: None,
                to: HardwareJobPhase::Completed,
            },
            Edge {
                id: "J04",
                from: HardwareJobPhase::Queued,
                event: HardwareJobEvent::ProviderReportedFailed,
                guard: None,
                to: HardwareJobPhase::Failed,
            },
            Edge {
                id: "J05",
                from: HardwareJobPhase::Queued,
                event: HardwareJobEvent::ProviderReportedCancelled,
                guard: None,
                to: HardwareJobPhase::Cancelled,
            },
            Edge {
                id: "J06",
                from: HardwareJobPhase::Queued,
                event: HardwareJobEvent::CancelAccepted,
                guard: None,
                to: HardwareJobPhase::Cancelled,
            },
            Edge {
                id: "J07",
                from: HardwareJobPhase::Running,
                event: HardwareJobEvent::ProviderReportedRunning,
                guard: None,
                to: HardwareJobPhase::Running,
            },
            Edge {
                id: "J08",
                from: HardwareJobPhase::Running,
                event: HardwareJobEvent::ProviderReportedCompleted,
                guard: None,
                to: HardwareJobPhase::Completed,
            },
            Edge {
                id: "J09",
                from: HardwareJobPhase::Running,
                event: HardwareJobEvent::ProviderReportedFailed,
                guard: None,
                to: HardwareJobPhase::Failed,
            },
            Edge {
                id: "J10",
                from: HardwareJobPhase::Running,
                event: HardwareJobEvent::ProviderReportedCancelled,
                guard: None,
                to: HardwareJobPhase::Cancelled,
            },
            Edge {
                id: "J11",
                from: HardwareJobPhase::Running,
                event: HardwareJobEvent::CancelAccepted,
                guard: None,
                to: HardwareJobPhase::Cancelled,
            },
        ],
    };

impl HardwareJobPhase {
    /// The one transition function of `HardwareJobMachine`.
    pub fn apply(self, event: HardwareJobEvent) -> Result<Self, TransitionRefused> {
        transition(&HARDWARE_JOB_MACHINE, self, event, |_| false).map(|edge| edge.to)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_types::transition_refused::TransitionRefusalReason;

    // spec: MEDULLA-CONTRACT-R062.2
    #[test]
    fn every_legal_transition_applies() {
        assert_eq!(HARDWARE_JOB_MACHINE.edges.len(), 11);
        for edge in HARDWARE_JOB_MACHINE.edges {
            assert_eq!(edge.from.apply(edge.event), Ok(edge.to), "{}", edge.id);
        }
        assert_eq!(
            HardwareJobPhase::of(&JobStatus::Failed("x".to_string())),
            HardwareJobPhase::Failed
        );
        assert_eq!(
            HardwareJobEvent::reported(&JobStatus::Cancelled),
            HardwareJobEvent::ProviderReportedCancelled
        );
    }

    // spec: MEDULLA-CONTRACT-R062.2
    #[test]
    fn every_illegal_pair_is_refused_with_transition_refused() {
        let machine = &HARDWARE_JOB_MACHINE;
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
                assert_eq!(
                    refused.reason,
                    expected,
                    "{} {}",
                    state.stored(),
                    event.name()
                );
                assert_eq!(refused.machine, "HardwareJobMachine");
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
}
