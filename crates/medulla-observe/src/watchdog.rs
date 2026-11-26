//! The stall watchdog (MEDULLA-OBSERVE-R005.2 and R005.3).
//!
//! One task registers as `observe-watchdog`. Every `observe.stall_check_interval_ms` it scans
//! the live tasks: a `Running` or `Progressing` task whose last progress is older than
//! `observe.task_stall_after_ms` receives `StallDetected` and logs at ERROR with its name,
//! owner, cursor, units, last progress location and age. A stalled task of kind
//! `Materialization` also degrades the readiness of its graph with the reason `NO_PROGRESS`
//! and the task's name, so the blocker of a graph that never becomes ready names the task that
//! stopped. A stalled task that reports progress again returns to `Progressing` through the
//! task machine's own edge.
//!
//! [`scan`] takes the time as an argument; only [`spawn_watchdog`] reads the clock.

use std::time::{SystemTime, UNIX_EPOCH};

use crate::machine::readiness::{DegradeReason, NO_PROGRESS};
use crate::registry::readiness::ReadinessRegistry;
use crate::registry::tasks::{ProgressCursor, TaskKind, TaskRegistry, TaskRow, TaskSpec};

/// The name the watchdog registers under.
pub const WATCHDOG_TASK_NAME: &str = "observe-watchdog";
/// The prefix of a graph page-in task: the graph's storage name follows it.
pub const PAGE_IN_TASK_PREFIX: &str = "lazy-page-in:";

/// The graph a `Materialization` task pages, from its name.
pub fn materialization_graph(task_name: &str) -> Option<&str> {
    task_name
        .strip_prefix(PAGE_IN_TASK_PREFIX)
        .filter(|graph| !graph.is_empty())
}

/// What one scan found.
#[derive(Clone, Debug)]
pub struct StalledTask {
    /// The task's row as the scan marked it.
    pub row: TaskRow,
    /// The graph whose readiness the scan degraded, for a `Materialization` task.
    pub degraded_graph: Option<String>,
}

/// One scan of `tasks` at `now_unix_ms`. A stalled `Materialization` task degrades its graph in
/// `readiness`. The watchdog's own task is never scanned.
pub fn scan(
    tasks: &TaskRegistry,
    readiness: &ReadinessRegistry,
    now_unix_ms: u64,
    stall_after_ms: u64,
) -> Vec<StalledTask> {
    tasks
        .detect_stalls(now_unix_ms, stall_after_ms)
        .into_iter()
        .filter(|row| row.name != WATCHDOG_TASK_NAME)
        .map(|row| {
            let degraded_graph = if row.kind == TaskKind::Materialization {
                materialization_graph(&row.name).and_then(|graph| {
                    let reason = DegradeReason {
                        code: NO_PROGRESS.to_string(),
                        task: Some(row.name.clone()),
                        detail: format!(
                            "task {} stalled after {} units",
                            row.name, row.units_done
                        ),
                    };
                    readiness
                        .degrade(graph, reason)
                        .ok()
                        .map(|_| graph.to_string())
                })
            } else {
                None
            };
            StalledTask { row, degraded_graph }
        })
        .collect()
}

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as u64)
}

/// Start the watchdog as the registered task `observe-watchdog`. It scans the process-wide task
/// registry at the interval and threshold the settings hold at each pass, and reports one unit
/// of progress per pass so the scan itself is visible in `Introspect tasks`.
#[track_caller]
pub fn spawn_watchdog() -> tokio::task::JoinHandle<()> {
    crate::spawn::spawn(
        TaskSpec {
            name: WATCHDOG_TASK_NAME.to_string(),
            owner: "observe".to_string(),
            kind: TaskKind::Maintenance,
        },
        watchdog_loop(),
    )
}

/// The watchdog's loop: sleep one interval, scan, report one pass. It ends only with the process.
async fn watchdog_loop() {
    let mut passes = 0_u64;
    loop {
        tokio::time::sleep(std::time::Duration::from_millis(
            crate::settings::stall_check_interval_ms(),
        ))
        .await;
        scan(
            crate::global(),
            crate::readiness(),
            unix_ms(),
            crate::settings::task_stall_after_ms(),
        );
        passes += 1;
        crate::spawn::progress(ProgressCursor::Counter(passes), passes);
    }
}
