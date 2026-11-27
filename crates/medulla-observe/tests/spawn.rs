//! `observe::spawn` registers a job, carries its progress and records a panic with its location.

use medulla_observe::machine::task::{TaskFailure, TaskState};
use medulla_observe::registry::tasks::{file_line, ProgressCursor, TaskKind, TaskSpec};

fn spec(name: &str) -> TaskSpec {
    TaskSpec {
        name: name.to_string(),
        owner: "tests".to_string(),
        kind: TaskKind::External,
    }
}

// spec: MEDULLA-OBSERVE-R003.2
#[tokio::test]
async fn spawn_registers_and_panic_is_recorded_with_location() {
    // A job that reports progress and completes.
    let done = medulla_observe::spawn::spawn(spec("spawn-ok"), async {
        assert!(medulla_observe::spawn::progress(
            ProgressCursor::Counter(7),
            7
        ));
    });
    done.await.unwrap();
    let row = medulla_observe::global().get("spawn-ok").unwrap();
    assert_eq!(row.state, TaskState::Completed);
    assert_eq!(row.units_done, 7);
    let at = file_line(row.last_progress_location.unwrap());
    assert!(at.contains("tests/spawn.rs"), "{at}");
    assert!(file_line(row.registered_location).contains("tests/spawn.rs"));

    // A job whose future returns a typed refusal.
    let refused = medulla_observe::spawn::spawn(spec("spawn-refused"), async {
        Err::<(), String>("STALE_VERSION: expected 3".to_string())
    });
    refused.await.unwrap();
    let row = medulla_observe::global().get("spawn-refused").unwrap();
    assert_eq!(row.state, TaskState::Failed);
    assert_eq!(
        row.failure.as_ref().map(TaskFailure::code),
        Some("STALE_VERSION")
    );

    // A panic finishes with TASK_PANICKED and the panic location; the process continues.
    let panicked = medulla_observe::spawn::spawn(spec("spawn-panics"), async {
        let value: Option<u32> = None;
        value.expect("the job panics here");
    });
    panicked.await.unwrap();
    let row = medulla_observe::global().get("spawn-panics").unwrap();
    assert_eq!(row.state, TaskState::Failed);
    match row.failure {
        Some(TaskFailure::Panicked { location }) => {
            assert!(location.contains("tests/spawn.rs"), "{location}")
        }
        other => panic!("TASK_PANICKED expected, got {other:?}"),
    }

    // Outside a job there is no reporter.
    assert!(!medulla_observe::spawn::progress(
        ProgressCursor::Counter(1),
        1
    ));
}
