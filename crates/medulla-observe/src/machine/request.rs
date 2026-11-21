//! `RequestMachine` (MEDULLA-OBSERVE-R004.3): the phase of one in-flight request.
//!
//! States: Received (initial), Admitted, Authorized, Handling, WaitingLock, Responding, Done
//! and Refused; Done and Refused are terminal. `Handling` and `WaitingLock` alternate while the
//! request holds or waits for a lock. [`RequestState::apply`] is the one transition function.
//! Every pair of state and event that no edge lists is refused with `TRANSITION_REFUSED`.
//! The edges Q01 to Q13 are in `specs/engine-observability/data-model.md`.

use medulla_types::transition_refused::{
    transition, Edge, MachineDef, MachineEvent, MachineState, TransitionRefused,
};
use serde::{Deserialize, Serialize};

/// The closed state set of `RequestMachine`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestState {
    /// The request arrived and nothing was decided.
    Received,
    /// Admission accepted the request.
    Admitted,
    /// The caller is authorized for the method.
    Authorized,
    /// The handler runs.
    Handling,
    /// The handler waits for a lock.
    WaitingLock,
    /// The response is being written.
    Responding,
    /// The response was written.
    Done,
    /// The request was refused before or while it ran.
    Refused,
}

/// The closed event set of `RequestMachine`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RequestEvent {
    /// Admission accepted the request.
    Admit,
    /// The caller was authorized.
    Authorize,
    /// The handler began.
    Handle,
    /// The handler began to wait for a lock.
    WaitLock,
    /// The handler got the lock.
    Acquire,
    /// The handler finished and the response is being written.
    Respond,
    /// The response was written.
    Complete,
    /// The request was refused.
    Refuse,
}

/// The guards of `RequestMachine`: none.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RequestGuard {}

impl MachineState for RequestState {
    fn stored(self) -> &'static str {
        match self {
            Self::Received => "received",
            Self::Admitted => "admitted",
            Self::Authorized => "authorized",
            Self::Handling => "handling",
            Self::WaitingLock => "waiting_lock",
            Self::Responding => "responding",
            Self::Done => "done",
            Self::Refused => "refused",
        }
    }
}

impl MachineEvent for RequestEvent {
    fn name(self) -> &'static str {
        match self {
            Self::Admit => "Admit",
            Self::Authorize => "Authorize",
            Self::Handle => "Handle",
            Self::WaitLock => "WaitLock",
            Self::Acquire => "Acquire",
            Self::Respond => "Respond",
            Self::Complete => "Complete",
            Self::Refuse => "Refuse",
        }
    }
}

const fn edge(
    id: &'static str,
    from: RequestState,
    event: RequestEvent,
    to: RequestState,
) -> Edge<RequestState, RequestEvent, RequestGuard> {
    Edge {
        id,
        from,
        event,
        guard: None,
        to,
    }
}

use RequestEvent as E;
use RequestState as S;

/// `RequestMachine`: the closed edge table Q01 to Q13. Every other pair is refused.
pub static REQUEST_MACHINE: MachineDef<RequestState, RequestEvent, RequestGuard> = MachineDef {
    name: "RequestMachine",
    states: &[
        S::Received,
        S::Admitted,
        S::Authorized,
        S::Handling,
        S::WaitingLock,
        S::Responding,
        S::Done,
        S::Refused,
    ],
    initial: S::Received,
    terminals: &[S::Done, S::Refused],
    events: &[
        E::Admit,
        E::Authorize,
        E::Handle,
        E::WaitLock,
        E::Acquire,
        E::Respond,
        E::Complete,
        E::Refuse,
    ],
    guards: &[],
    edges: &[
        edge("Q01", S::Received, E::Admit, S::Admitted),
        edge("Q02", S::Received, E::Refuse, S::Refused),
        edge("Q03", S::Admitted, E::Authorize, S::Authorized),
        edge("Q04", S::Admitted, E::Refuse, S::Refused),
        edge("Q05", S::Authorized, E::Handle, S::Handling),
        edge("Q06", S::Authorized, E::Refuse, S::Refused),
        edge("Q07", S::Handling, E::WaitLock, S::WaitingLock),
        edge("Q08", S::WaitingLock, E::Acquire, S::Handling),
        edge("Q09", S::Handling, E::Respond, S::Responding),
        edge("Q10", S::Handling, E::Refuse, S::Refused),
        edge("Q11", S::WaitingLock, E::Refuse, S::Refused),
        edge("Q12", S::Responding, E::Complete, S::Done),
        edge("Q13", S::Responding, E::Refuse, S::Refused),
    ],
};

impl RequestState {
    /// Every state, in declaration order.
    pub const ALL: [RequestState; 8] = [
        S::Received,
        S::Admitted,
        S::Authorized,
        S::Handling,
        S::WaitingLock,
        S::Responding,
        S::Done,
        S::Refused,
    ];

    /// The state a new request takes.
    pub const fn initial() -> Self {
        S::Received
    }

    /// Whether the state accepts no further event.
    pub fn is_terminal(self) -> bool {
        REQUEST_MACHINE.is_terminal(self)
    }

    /// The one transition function of `RequestMachine`: the next state, or `TRANSITION_REFUSED`.
    pub fn apply(self, event: RequestEvent) -> Result<Self, TransitionRefused> {
        transition(&REQUEST_MACHINE, self, event, |guard| match guard {}).map(|edge| edge.to)
    }
}
