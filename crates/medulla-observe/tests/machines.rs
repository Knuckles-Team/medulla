//! The explicit state machines of the runtime registries.

use medulla_observe::machine::readiness::{
    ReadinessEvent, ReadinessState, READINESS_MACHINE,
};
use medulla_observe::machine::request::{RequestEvent, RequestState, REQUEST_MACHINE};
use medulla_observe::events::{EventRing, TransitionEvent, MAX_SUBJECT_BYTES};
use medulla_observe::machine::task::{TaskEvent, TaskState, TASK_MACHINE};
use medulla_observe::registry::readiness::ReadinessRegistry;
use medulla_observe::registry::tasks::{Outcome, ProgressCursor, TaskKind, TaskRegistry, TaskSpec};
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
// Proves RequestMachine edges Q01 to Q13, that Handling and WaitingLock alternate, and that every other pair is refused.
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

const READINESS_EDGES: [(&str, ReadinessState, ReadinessEvent, ReadinessState); 12] = [
    ("D01", ReadinessState::Opening, ReadinessEvent::BeginMigrate, ReadinessState::Migrating),
    ("D02", ReadinessState::Opening, ReadinessEvent::BeginMaterialize, ReadinessState::Materializing),
    ("D03", ReadinessState::Opening, ReadinessEvent::Materialized, ReadinessState::Ready),
    ("D04", ReadinessState::Migrating, ReadinessEvent::BeginMaterialize, ReadinessState::Materializing),
    ("D05", ReadinessState::Materializing, ReadinessEvent::Materialized, ReadinessState::Ready),
    ("D06", ReadinessState::Opening, ReadinessEvent::Degrade, ReadinessState::Degraded),
    ("D07", ReadinessState::Migrating, ReadinessEvent::Degrade, ReadinessState::Degraded),
    ("D08", ReadinessState::Materializing, ReadinessEvent::Degrade, ReadinessState::Degraded),
    ("D09", ReadinessState::Ready, ReadinessEvent::Degrade, ReadinessState::Degraded),
    ("D10", ReadinessState::Degraded, ReadinessEvent::Reopen, ReadinessState::Opening),
    ("D11", ReadinessState::Ready, ReadinessEvent::Reopen, ReadinessState::Opening),
    ("D12", ReadinessState::Degraded, ReadinessEvent::Degrade, ReadinessState::Degraded),
];

const READINESS_EVENTS: [ReadinessEvent; 5] = [
    ReadinessEvent::BeginMigrate,
    ReadinessEvent::BeginMaterialize,
    ReadinessEvent::Materialized,
    ReadinessEvent::Degrade,
    ReadinessEvent::Reopen,
];

// spec: MEDULLA-OBSERVE-R004.2
#[test]
fn readiness_machine_edges_d01_to_d12() {
    assert_eq!(READINESS_MACHINE.name, "ReadinessMachine");
    assert_eq!(READINESS_MACHINE.initial, ReadinessState::Opening);
    assert_eq!(ReadinessState::initial(), ReadinessState::Opening);
    assert_eq!(READINESS_MACHINE.edges.len(), READINESS_EDGES.len());
    for (id, from, event, to) in READINESS_EDGES {
        let edge = READINESS_MACHINE
            .edges
            .iter()
            .find(|edge| edge.id == id)
            .unwrap_or_else(|| panic!("edge {id} is missing"));
        assert_eq!((edge.from, edge.event, edge.to), (from, event, to), "{id}");
        assert_eq!(from.apply(event), Ok(to), "{id}");
    }
    // No state is terminal: Ready and Degraded accept Reopen and Degrade, so a later fault
    // changes the state.
    assert!(READINESS_MACHINE.terminals.is_empty());
    assert_eq!(
        ReadinessState::Ready.apply(ReadinessEvent::Degrade),
        Ok(ReadinessState::Degraded)
    );
    assert_eq!(
        ReadinessState::Degraded.apply(ReadinessEvent::Reopen),
        Ok(ReadinessState::Opening)
    );
    for from in ReadinessState::ALL {
        for event in READINESS_EVENTS {
            if READINESS_EDGES
                .iter()
                .any(|edge| edge.1 == from && edge.2 == event)
            {
                continue;
            }
            let refused = from.apply(event).expect_err("an unlisted pair is refused");
            assert_eq!(refused.machine, "ReadinessMachine");
            assert_eq!(refused.reason, TransitionRefusalReason::IllegalTransition);
        }
    }
}

/// The observe state types the scan guards.
const STATE_TYPES: [&str; 3] = ["TaskState", "ReadinessState", "RequestState"];

/// True when `line` writes a `.state` or `.phase` field with a right side that is not the result
/// of `apply`, `initial` or the `next` state taken from one of them.
fn assigns_state_outside_apply(line: &str) -> bool {
    for field in [".state", ".phase"] {
        let mut from = 0;
        while let Some(found) = line[from..].find(field) {
            let after = from + found + field.len();
            from = after;
            let rest = line[after..].trim_start();
            if let Some(rhs) = rest.strip_prefix('=') {
                if rhs.starts_with('=') {
                    continue;
                }
                if !(rhs.contains("apply") || rhs.contains("initial") || rhs.contains("next")) {
                    return true;
                }
            }
        }
    }
    false
}

/// True when `line` names a variant of a state type as a value: `TaskState::Running`, not
/// `TaskState::initial()` and not a comparison or a match pattern.
fn creates_state_value(line: &str) -> bool {
    const CONTEXT: [&str; 5] = ["==", "!=", "=>", "|", "matches!"];
    if CONTEXT.iter().any(|mark| line.contains(mark)) || line.contains("assert") {
        return false;
    }
    for name in STATE_TYPES {
        let needle = format!("{name}::");
        let mut from = 0;
        while let Some(found) = line[from..].find(&needle) {
            let at = from + found;
            from = at + needle.len();
            let boundary = line[..at]
                .chars()
                .next_back()
                .is_none_or(|c| !(c.is_alphanumeric() || c == '_'));
            let mut tail = line[from..].chars();
            let variant = tail.next().is_some_and(|c| c.is_ascii_uppercase())
                && tail.next().is_some_and(|c| c.is_ascii_lowercase());
            if boundary && variant {
                return true;
            }
        }
    }
    false
}

/// Violations of one source file outside the machine directory, as `line: text`.
fn state_writer_violations(source: &str) -> Vec<String> {
    source
        .lines()
        .enumerate()
        .filter(|(_, line)| !line.trim_start().starts_with("//"))
        .filter(|(_, line)| assigns_state_outside_apply(line) || creates_state_value(line))
        .map(|(index, line)| format!("{}: {}", index + 1, line.trim()))
        .collect()
}

/// Every first-party production source that uses the observe machines: the observe sources
/// outside `src/machine/`, and any source that names `medulla_observe`. Test sources are not
/// production writers and are skipped.
fn machine_user_sources() -> Vec<(std::path::PathBuf, String)> {
    fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if path.is_dir() {
                if name.starts_with("target") || name == "tests" || name == "benches" {
                    continue;
                }
                walk(&path, out);
            } else if name.ends_with(".rs") && !name.ends_with("_tests.rs") && name != "tests.rs" {
                out.push(path);
            }
        }
    }
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let machine_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/machine");
    let mut files = Vec::new();
    for top in ["crates", "src"] {
        walk(&root.join(top), &mut files);
    }
    files
        .into_iter()
        .filter(|path| {
            let text = path.to_string_lossy();
            !path.starts_with(&machine_dir) && !text.contains("-tests/")
        })
        .filter_map(|path| {
            let source = std::fs::read_to_string(&path).ok()?;
            let observe_own = path.starts_with(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
            );
            (observe_own || source.contains("medulla_observe")).then_some((path, source))
        })
        .collect()
}

// spec: MEDULLA-OBSERVE-R004.6
#[test]
fn no_writer_sets_state_outside_apply() {
    // The scan refuses a known-bad input: a gate that has only ever passed proves nothing.
    assert!(!state_writer_violations("row.state = ReadinessState::Ready;").is_empty());
    assert!(!state_writer_violations("let next = TaskState::Running;").is_empty());
    assert!(!state_writer_violations("row.phase = RequestState::Done;").is_empty());
    assert!(state_writer_violations("row.state = row.state.apply(event)?;").is_empty());
    assert!(state_writer_violations("state: ReadinessState::initial(),").is_empty());
    assert!(state_writer_violations("if row.state == TaskState::Registered {").is_empty());
    assert!(state_writer_violations("ChangeRequestState::Open").is_empty());

    // An empty universe fails: the scan paths drifted or the machines were deleted.
    let sources = machine_user_sources();
    assert!(
        sources.len() >= 10,
        "the scan found only {} sources that use the machines",
        sources.len()
    );
    assert!(
        sources
            .iter()
            .any(|(path, _)| path.ends_with("registry/readiness.rs")),
        "the readiness registry, the one writer of readiness state, was not scanned"
    );
    let mut found = Vec::new();
    for (path, source) in &sources {
        for violation in state_writer_violations(source) {
            found.push(format!("{}:{violation}", path.display()));
        }
    }
    assert!(
        found.is_empty(),
        "code outside crates/medulla-observe/src/machine/ sets a machine state other than through apply or initial:\n{}",
        found.join("\n")
    );
}

// spec: MEDULLA-OBSERVE-R004.4
#[test]
fn accepted_and_refused_transitions_are_recorded() {
    // A task's lifecycle appends one event per accepted transition, in order, with the stored
    // text of both states and the event name.
    let tasks = TaskRegistry::new(8);
    let name = "events-test:lifecycle-task";
    let handle = tasks.register(TaskSpec {
        name: name.to_string(),
        owner: "test".to_string(),
        kind: TaskKind::External,
    });
    handle.progress(ProgressCursor::Counter(1), 1);
    handle.finish(Outcome::Completed);
    let mine: Vec<(u64, TransitionEvent)> = medulla_observe::events()
        .snapshot()
        .into_iter()
        .filter(|(_, event)| event.subject == name)
        .collect();
    let steps: Vec<(&str, &str, &str)> = mine
        .iter()
        .map(|(_, event)| (event.from, event.event, event.to))
        .collect();
    assert_eq!(
        steps,
        vec![
            ("registered", "Start", "running"),
            ("running", "Progress", "progressing"),
            ("progressing", "Finish", "completed"),
        ]
    );
    assert!(mine.iter().all(|(_, event)| event.machine == "TaskMachine"));
    assert!(mine.iter().all(|(_, event)| event.at_unix_ms > 0));
    assert!(mine.windows(2).all(|pair| pair[0].0 < pair[1].0));

    // A refused transition leaves the state unchanged, is kept in the refusal ring with the
    // fields of the refusal, and is counted; it is not an accepted event.
    let readiness = ReadinessRegistry::new();
    let graph = "events-test:graph";
    readiness.apply(graph, ReadinessEvent::Materialized, None).unwrap();
    let refused_before = medulla_observe::events().refused_total();
    let refused = readiness
        .apply(graph, ReadinessEvent::BeginMigrate, None)
        .expect_err("Ready does not begin a migration");
    assert_eq!(refused.machine, "ReadinessMachine");
    assert_eq!(refused.from, "ready");
    assert_eq!(readiness.get(graph).unwrap().state, ReadinessState::Ready);
    assert_eq!(medulla_observe::events().refused_total(), refused_before + 1);
    let kept = medulla_observe::events()
        .refused()
        .into_iter()
        .find(|entry| entry.subject == graph)
        .expect("the refusal is kept");
    assert_eq!(kept.refused, refused);
    assert!(medulla_observe::events()
        .snapshot()
        .iter()
        .all(|(_, event)| !(event.subject == graph && event.event == "BeginMigrate")));

    // The ring is bounded and keeps the newest events; a subject is cut to a fixed size.
    let ring = EventRing::new(3);
    for index in 0..5u64 {
        ring.push(TransitionEvent {
            machine: "TaskMachine",
            subject: "x".repeat(300),
            from: "running",
            event: "Progress",
            to: "progressing",
            at_unix_ms: index,
            trace_id: None,
        });
    }
    assert_eq!(ring.len(), 3);
    assert_eq!(ring.sequence(), 5);
    let held = ring.snapshot();
    assert_eq!(held.iter().map(|(seq, _)| *seq).collect::<Vec<_>>(), vec![3, 4, 5]);
    assert!(held.iter().all(|(_, event)| event.subject.len() == MAX_SUBJECT_BYTES));
    assert_eq!(ring.last(2).len(), 2);
    assert_eq!(ring.last(2)[0].0, 4);
}
