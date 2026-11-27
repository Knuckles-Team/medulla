//! The task registry, the spawn helper and the progress guard of a job.

use medulla_observe::machine::task::{TaskFailure, TaskState};
use medulla_observe::registry::tasks::{
    file_line, Outcome, ProgressCursor, TaskKind, TaskRegistry, TaskSpec,
};

fn spec(name: &str) -> TaskSpec {
    TaskSpec {
        name: name.to_string(),
        owner: "tests".to_string(),
        kind: TaskKind::External,
    }
}

// spec: MEDULLA-OBSERVE-R003.1
#[test]
fn register_progress_finish_and_drop_record_states() {
    let registry = TaskRegistry::new(8);

    let handle = registry.register(spec("finishes"));
    assert_eq!(
        registry.get("finishes").unwrap().state,
        TaskState::Registered
    );
    handle.progress(ProgressCursor::Counter(3), 3);
    let row = registry.get("finishes").unwrap();
    assert_eq!(row.state, TaskState::Progressing);
    assert_eq!(row.cursor, Some(ProgressCursor::Counter(3)));
    assert_eq!(row.units_done, 3);
    assert!(row.last_progress_unix_ms > 0);
    // The caller location of `progress` is this file, not the registry's.
    let progress_at = file_line(row.last_progress_location.unwrap());
    assert!(
        progress_at.contains("tests/task_registry.rs"),
        "{progress_at}"
    );
    let registered_at = file_line(row.registered_location);
    assert!(
        registered_at.contains("tests/task_registry.rs"),
        "{registered_at}"
    );
    // Units never decrease.
    handle.progress(ProgressCursor::Counter(4), 1);
    assert_eq!(registry.get("finishes").unwrap().units_done, 3);
    handle.finish(Outcome::Completed);
    let row = registry.get("finishes").unwrap();
    assert_eq!(row.state, TaskState::Completed);
    assert!(registry.live().is_empty());

    let handle = registry.register(spec("fails"));
    handle.finish(Outcome::Failed(TaskFailure::Refused {
        code: "STALE_VERSION".to_string(),
    }));
    let row = registry.get("fails").unwrap();
    assert_eq!(row.state, TaskState::Failed);
    assert_eq!(
        row.failure.as_ref().map(TaskFailure::code),
        Some("STALE_VERSION")
    );

    let handle = registry.register(spec("cancelled"));
    handle.finish(Outcome::Cancelled);
    assert_eq!(
        registry.get("cancelled").unwrap().state,
        TaskState::Cancelled
    );

    let handle = registry.register(spec("dropped"));
    handle.progress(ProgressCursor::opaque("page 2"), 1);
    drop(handle);
    let row = registry.get("dropped").unwrap();
    assert_eq!(row.state, TaskState::Failed);
    assert_eq!(row.failure, Some(TaskFailure::Dropped));
    assert_eq!(
        row.failure.as_ref().map(TaskFailure::code),
        Some("TASK_DROPPED")
    );
}

// spec: MEDULLA-OBSERVE-R003.1
#[test]
fn a_full_registry_and_a_held_name_still_run_the_job_and_count_the_drop() {
    let registry = TaskRegistry::new(2);
    let first = registry.register(spec("a"));
    let second = registry.register(spec("b"));
    assert_eq!(registry.dropped_total(), 0);

    // Over capacity: the handle works, the task is not listed, the drop is counted.
    let overflow = registry.register(spec("c"));
    overflow.progress(ProgressCursor::Counter(1), 1);
    assert_eq!(overflow.row().state, TaskState::Progressing);
    assert!(registry.get("c").is_none());
    assert_eq!(registry.dropped_total(), 1);

    // A live name is unique.
    let duplicate = registry.register(spec("a"));
    assert_eq!(registry.dropped_total(), 2);
    drop(duplicate);
    assert_eq!(registry.get("a").unwrap().state, TaskState::Registered);

    let names: Vec<String> = registry.live().into_iter().map(|row| row.name).collect();
    assert_eq!(names, ["a", "b"]);
    first.finish(Outcome::Completed);
    second.finish(Outcome::Completed);
    // A finished task frees its slot.
    let again = registry.register(spec("c"));
    assert!(registry.get("c").is_some());
    again.finish(Outcome::Completed);
}

// spec: MEDULLA-OBSERVE-R003.1
#[test]
fn names_and_opaque_cursors_are_bounded() {
    let registry = TaskRegistry::new(4);
    let long = "é".repeat(100);
    let handle = registry.register(spec(&long));
    let row = handle.row();
    assert!(row.name.len() <= 128 && row.name.chars().all(|c| c == 'é'));
    handle.progress(ProgressCursor::opaque("é".repeat(200)), 1);
    match handle.row().cursor {
        Some(ProgressCursor::Opaque(value)) => {
            assert!(value.len() <= 256 && value.chars().all(|c| c == 'é'))
        }
        other => panic!("opaque cursor expected, got {other:?}"),
    }
}
