//! The stall watchdog: an idle task goes `Stalled` and returns to `Progressing` when it reports
//! progress again. The scan takes the time as an argument, so the test never waits.

use medulla_observe::machine::readiness::{ReadinessState, NO_PROGRESS};
use medulla_observe::machine::task::TaskState;
use medulla_observe::registry::readiness::ReadinessRegistry;
use medulla_observe::registry::tasks::{ProgressCursor, TaskKind, TaskRegistry, TaskSpec};
use medulla_observe::watchdog::{materialization_graph, scan};

const STALL_AFTER_MS: u64 = 500;

fn spec(name: &str, kind: TaskKind) -> TaskSpec {
    TaskSpec {
        name: name.to_string(),
        owner: "tests".to_string(),
        kind,
    }
}

// spec: MEDULLA-OBSERVE-R005.2
#[test]
fn idle_task_goes_stalled_and_recovers_on_progress() {
    let tasks = TaskRegistry::new(16);
    let readiness = ReadinessRegistry::new();
    let handle = tasks.register(spec("watchdog-test:idle", TaskKind::External));
    handle.progress(ProgressCursor::Counter(1), 1);
    let progressed_at = handle.row().last_progress_unix_ms;
    assert_eq!(handle.row().state, TaskState::Progressing);

    // Within the threshold nothing is marked, whatever the clock.
    assert!(scan(
        &tasks,
        &readiness,
        progressed_at + STALL_AFTER_MS,
        STALL_AFTER_MS
    )
    .is_empty());
    assert_eq!(handle.row().state, TaskState::Progressing);

    // Past the threshold the task is Stalled; the scan reports its row.
    let found = scan(
        &tasks,
        &readiness,
        progressed_at + STALL_AFTER_MS + 1,
        STALL_AFTER_MS,
    );
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].row.name, "watchdog-test:idle");
    assert_eq!(found[0].row.state, TaskState::Stalled);
    assert_eq!(found[0].degraded_graph, None);
    assert_eq!(handle.row().state, TaskState::Stalled);

    // A stalled task is not marked twice.
    assert!(scan(
        &tasks,
        &readiness,
        progressed_at + 10 * STALL_AFTER_MS,
        STALL_AFTER_MS
    )
    .is_empty());

    // Progress returns it to Progressing, and the next scan finds it healthy.
    handle.progress(ProgressCursor::Counter(2), 2);
    assert_eq!(handle.row().state, TaskState::Progressing);
    let again = handle.row().last_progress_unix_ms;
    assert!(scan(&tasks, &readiness, again, STALL_AFTER_MS).is_empty());

    // A task that never reported progress has no age and is not scanned.
    let silent = tasks.register(spec("watchdog-test:silent", TaskKind::External));
    assert!(scan(&tasks, &readiness, u64::MAX, STALL_AFTER_MS)
        .iter()
        .all(|stalled| stalled.row.name != "watchdog-test:silent"));
    drop(silent);
}

// spec: MEDULLA-OBSERVE-R005.3
#[test]
fn stalled_materialization_task_degrades_its_graph_with_the_task_name() {
    let tasks = TaskRegistry::new(16);
    let readiness = ReadinessRegistry::new();
    assert_eq!(materialization_graph("lazy-page-in:sales"), Some("sales"));
    assert_eq!(materialization_graph("lazy-page-in:"), None);
    assert_eq!(materialization_graph("other:sales"), None);

    readiness.open("sales");
    let handle = tasks.register(spec("lazy-page-in:sales", TaskKind::Materialization));
    handle.progress(ProgressCursor::Counter(1), 1);
    let at = handle.row().last_progress_unix_ms;
    let found = scan(&tasks, &readiness, at + STALL_AFTER_MS + 1, STALL_AFTER_MS);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].degraded_graph.as_deref(), Some("sales"));
    let row = readiness.get("sales").unwrap();
    assert_eq!(row.state, ReadinessState::Degraded);
    let reason = row.reason.expect("a degraded graph carries its reason");
    assert_eq!(reason.code, NO_PROGRESS);
    assert_eq!(reason.task.as_deref(), Some("lazy-page-in:sales"));

    // A stalled task of another kind degrades no graph.
    readiness.open("other");
    let index = tasks.register(spec("lazy-page-in:other", TaskKind::Index));
    index.progress(ProgressCursor::Counter(1), 1);
    let at = index.row().last_progress_unix_ms;
    let found = scan(&tasks, &readiness, at + STALL_AFTER_MS + 1, STALL_AFTER_MS);
    assert!(found.iter().all(|stalled| stalled.degraded_graph.is_none()));
    assert_eq!(
        readiness.get("other").unwrap().state,
        ReadinessState::Opening
    );
}

// spec: MEDULLA-OBSERVE-R005.2
#[test]
fn idle_heartbeat_is_not_a_stall_and_silence_is() {
    let tasks = TaskRegistry::new(16);
    let readiness = ReadinessRegistry::new();
    let waiting = tasks.register(spec("watchdog-test:waiting", TaskKind::Maintenance));
    let silent = tasks.register(spec("watchdog-test:hung", TaskKind::Maintenance));
    waiting.progress(ProgressCursor::Counter(1), 1);
    silent.progress(ProgressCursor::Counter(1), 1);
    let start = waiting
        .row()
        .last_progress_unix_ms
        .max(silent.row().last_progress_unix_ms);

    // The waiting loop sends heartbeats on the controlled clock across 4 thresholds.
    for step in 1..=4 {
        waiting.heartbeat_at(start + step * STALL_AFTER_MS);
    }
    let now = start + 4 * STALL_AFTER_MS + 1;
    let found = scan(&tasks, &readiness, now, STALL_AFTER_MS);

    // Only the task that stopped reporting is flagged; the idle one is not.
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].row.name, "watchdog-test:hung");
    assert_eq!(waiting.row().state, TaskState::Progressing);
    // A heartbeat is not work done: cursor and units are untouched.
    assert_eq!(waiting.row().units_done, 1);

    // Once the idle loop also goes silent past the threshold, it is flagged too.
    let later = scan(&tasks, &readiness, now + STALL_AFTER_MS + 1, STALL_AFTER_MS);
    assert_eq!(later.len(), 1);
    assert_eq!(later[0].row.name, "watchdog-test:waiting");

    // A heartbeat returns a stalled task to Progressing.
    waiting.heartbeat_at(now + 3 * STALL_AFTER_MS);
    assert_eq!(waiting.row().state, TaskState::Progressing);
    drop(silent);
}
