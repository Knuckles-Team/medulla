//! Fault injection for the chaos proof (MEDULLA-OBSERVE-R012).
//!
//! Three faults prove that a stalled task, a spinning loop and a frozen paging cursor are each
//! named in bounded time: `StallTask` registers a task that reports progress once and never
//! again, `SpinLoop` runs a loop that never advances its cursor behind a progress guard (or, with
//! the guard bypassed, pins one worker), and `FreezeCursor` holds the paging cursor of one graph
//! where it is. The faults exist only in a build with the `chaos` feature; the one hook the
//! engine calls, [`cursor_frozen`], answers `false` in every other build.

use std::collections::BTreeSet;

use parking_lot::Mutex;

static FROZEN: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());

/// Whether the paging cursor of `graph` is frozen: the one hook the paging loop calls. Always
/// `false` without the `chaos` feature.
pub fn cursor_frozen(graph: &str) -> bool {
    cfg!(feature = "chaos") && FROZEN.lock().contains(graph)
}

#[cfg(feature = "chaos")]
pub use faults::{freeze_cursor, spin_guarded, spin_pinned, stall_task, thaw_cursor};

#[cfg(feature = "chaos")]
mod faults {
    use crate::progress::{NoProgress, ProgressGuard};
    use crate::registry::tasks::{ProgressCursor, TaskHandle, TaskKind, TaskSpec};

    /// The fault `FreezeCursor`: hold the paging cursor of `graph` where it is, so every page
    /// the loop fetches ends at the cursor it started from. [`thaw_cursor`] ends the fault.
    pub fn freeze_cursor(graph: &str) {
        super::FROZEN.lock().insert(graph.to_string());
    }

    /// End the fault `FreezeCursor` for `graph`.
    pub fn thaw_cursor(graph: &str) {
        super::FROZEN.lock().remove(graph);
    }

    /// The fault `StallTask { name }`: register the task `name` in the process-wide registry,
    /// report progress once and never again. A name that starts with the page-in prefix makes
    /// the task a `Materialization` of the graph after the prefix. The task stays listed until
    /// the returned handle drops. The registration location is the caller's.
    #[track_caller]
    pub fn stall_task(name: &str) -> TaskHandle {
        let kind = if crate::watchdog::materialization_graph(name).is_some() {
            TaskKind::Materialization
        } else {
            TaskKind::External
        };
        let handle = crate::global().register(TaskSpec {
            name: name.to_string(),
            owner: "chaos".to_string(),
            kind,
        });
        handle.progress(ProgressCursor::Counter(1), 1);
        handle
    }

    /// The fault `SpinLoop { name }` behind its guard: a loop whose cursor never advances. It
    /// ends with `NO_PROGRESS` naming `name` and the `file:line` of the guard's creation, which
    /// is the caller's, after `observe.progress_idle_limit` equal cursors. It yields to the
    /// runtime on every pass.
    #[track_caller]
    pub fn spin_guarded(
        name: &str,
    ) -> impl std::future::Future<Output = Result<(), NoProgress>> + Send {
        let mut guard = ProgressGuard::new(name);
        async move {
            loop {
                guard.check(&ProgressCursor::Counter(0))?;
                tokio::task::yield_now().await;
            }
        }
    }

    /// The same fault with the guard bypassed: a loop that pins one runtime worker until `stop`
    /// is set. `Introspect runtime` flags the worker `spinning` and a profile shows its frames.
    pub fn spin_pinned(
        stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                std::hint::spin_loop();
            }
        })
    }
}
