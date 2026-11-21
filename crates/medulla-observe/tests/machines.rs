//! The explicit state machines of the runtime registries.

use medulla_observe::machine::request::{RequestEvent, RequestState, REQUEST_MACHINE};
use medulla_observe::machine::task::{TaskEvent, TaskState, TASK_MACHINE};
use medulla_types::transition_refused::{MachineState, TransitionRefusalReason};

use TaskEvent as E;
use TaskState as S;

/// The edge table of `specs/engine-observability/data-model.md`, written out once more here so
/// a change to the machine's table fails this test.
const EDGES: [(&str, TaskState, TaskEvent, TaskState); 17] = [
    ("T01", S::Registered, E::Start, S::Running),
    ("T02", S::Running, E::Progress, S::Progressing),
    ("T03", S::Progressing, E::Progress, S::Progressing),
    ("T04", S::Running, E::StallDetected, S::Stalled),
    ("T05", S::Progressing, E::StallDetected, S::Stalled),
    ("T06", S::Stalled, E::Progress, S::Progressing),
    ("T07", S::Running, E::Finish, S::Completed),
    ("T08", S::Progressing, E::Finish, S::Completed),
    ("T09", S::Stalled, E::Finish, S::Completed),
    ("T10", S::Registered, E::Fail, S::Failed),
    ("T11", S::Running, E::Fail, S::Failed),
    ("T12", S::Progressing, E::Fail, S::Failed),
    ("T13", S::Stalled, E::Fail, S::Failed),
    ("T14", S::Registered, E::Cancel, S::Cancelled),
    ("T15", S::Running, E::Cancel, S::Cancelled),
    ("T16", S::Progressing, E::Cancel, S::Cancelled),
    ("T17", S::Stalled, E::Cancel, S::Cancelled),
];

const EVENTS: [TaskEvent; 6] = [
    E::Start,
    E::Progress,
    E::StallDetected,
    E::Finish,
    E::Fail,
    E::Cancel,
];

// spec: MEDULLA-OBSERVE-R004.1
#[test]
fn task_machine_edges_t01_to_t17() {
    assert_eq!(TASK_MACHINE.name, "TaskMachine");
    assert_eq!(TASK_MACHINE.initial, S::Registered);
    assert_eq!(TASK_MACHINE.edges.len(), EDGES.len());
    for (id, from, event, to) in EDGES {
        let edge = TASK_MACHINE
            .edges
            .iter()
            .find(|edge| edge.id == id)
            .unwrap_or_else(|| panic!("edge {id} is missing"));
        assert_eq!((edge.from, edge.event, edge.to), (from, event, to), "{id}");
        assert_eq!(from.apply(event), Ok(to), "{id}");
    }
}

// spec: MEDULLA-OBSERVE-R004.1
#[test]
fn every_other_pair_of_state_and_event_is_refused() {
    for from in TaskState::ALL {
        for event in EVENTS {
            if EDGES.iter().any(|edge| edge.1 == from && edge.2 == event) {
                continue;
            }
            let refused = from
                .apply(event)
                .expect_err("a pair no edge lists must be refused");
            assert_eq!(refused.machine, "TaskMachine");
            assert_eq!(refused.from, from.stored());
            assert_eq!(refused.event, medulla_types::transition_refused::MachineEvent::name(event));
            let reason = if from.is_terminal() {
                TransitionRefusalReason::TerminalState
            } else {
                TransitionRefusalReason::IllegalTransition
            };
            assert_eq!(refused.reason, reason, "{from:?} {event:?}");
        }
    }
    let terminal: Vec<TaskState> = TaskState::ALL
        .into_iter()
        .filter(|state| state.is_terminal())
        .collect();
    assert_eq!(terminal, vec![S::Completed, S::Failed, S::Cancelled]);
}

const REQUEST_EDGES: [(&str, RequestState, RequestEvent, RequestState); 13] = [
    ("Q01", RequestState::Received, RequestEvent::Admit, RequestState::Admitted),
    ("Q02", RequestState::Received, RequestEvent::Refuse, RequestState::Refused),
    ("Q03", RequestState::Admitted, RequestEvent::Authorize, RequestState::Authorized),
    ("Q04", RequestState::Admitted, RequestEvent::Refuse, RequestState::Refused),
    ("Q05", RequestState::Authorized, RequestEvent::Handle, RequestState::Handling),
    ("Q06", RequestState::Authorized, RequestEvent::Refuse, RequestState::Refused),
    ("Q07", RequestState::Handling, RequestEvent::WaitLock, RequestState::WaitingLock),
    ("Q08", RequestState::WaitingLock, RequestEvent::Acquire, RequestState::Handling),
    ("Q09", RequestState::Handling, RequestEvent::Respond, RequestState::Responding),
    ("Q10", RequestState::Handling, RequestEvent::Refuse, RequestState::Refused),
    ("Q11", RequestState::WaitingLock, RequestEvent::Refuse, RequestState::Refused),
    ("Q12", RequestState::Responding, RequestEvent::Complete, RequestState::Done),
    ("Q13", RequestState::Responding, RequestEvent::Refuse, RequestState::Refused),
];

const REQUEST_EVENTS: [RequestEvent; 8] = [
    RequestEvent::Admit,
    RequestEvent::Authorize,
    RequestEvent::Handle,
    RequestEvent::WaitLock,
    RequestEvent::Acquire,
    RequestEvent::Respond,
    RequestEvent::Complete,
    RequestEvent::Refuse,
];

// spec: MEDULLA-OBSERVE-R004.3
#[test]
fn request_machine_edges_q01_to_q13() {
    assert_eq!(REQUEST_MACHINE.name, "RequestMachine");
    assert_eq!(REQUEST_MACHINE.initial, RequestState::Received);
    assert_eq!(REQUEST_MACHINE.edges.len(), REQUEST_EDGES.len());
    for (id, from, event, to) in REQUEST_EDGES {
        let edge = REQUEST_MACHINE
            .edges
            .iter()
            .find(|edge| edge.id == id)
            .unwrap_or_else(|| panic!("edge {id} is missing"));
        assert_eq!((edge.from, edge.event, edge.to), (from, event, to), "{id}");
        assert_eq!(from.apply(event), Ok(to), "{id}");
    }
    // Handling and WaitingLock alternate while a lock is held or waited for.
    let waiting = RequestState::Handling.apply(RequestEvent::WaitLock).unwrap();
    assert_eq!(waiting.apply(RequestEvent::Acquire), Ok(RequestState::Handling));
    for from in RequestState::ALL {
        for event in REQUEST_EVENTS {
            if REQUEST_EDGES.iter().any(|edge| edge.1 == from && edge.2 == event) {
                continue;
            }
            let refused = from.apply(event).expect_err("an unlisted pair is refused");
            assert_eq!(refused.machine, "RequestMachine");
            let reason = if from.is_terminal() {
                TransitionRefusalReason::TerminalState
            } else {
                TransitionRefusalReason::IllegalTransition
            };
            assert_eq!(refused.reason, reason, "{from:?} {event:?}");
        }
    }
    let terminal: Vec<RequestState> = RequestState::ALL
        .into_iter()
        .filter(|state| state.is_terminal())
        .collect();
    assert_eq!(terminal, vec![RequestState::Done, RequestState::Refused]);
}
