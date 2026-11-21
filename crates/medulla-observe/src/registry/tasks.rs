//! The task registry (MEDULLA-OBSERVE-R003.1).
//!
//! [`TaskRegistry::register`] lists one background job and returns its [`TaskHandle`].
//! The handle records progress ([`TaskHandle::progress`]) and ends the job
//! ([`TaskHandle::finish`]). A dropped handle without `finish` records
//! `Failed(TASK_DROPPED)`. Every state change goes through
//! [`TaskState::apply`](crate::machine::task::TaskState::apply): `apply_event` below is the
//! one writer of a task's state.
//!
//! The registry holds at most `capacity` live entries (`observe.task_registry_capacity`).
//! A job that does not fit, or whose name a live job already holds, still runs: its handle
//! works and is not listed, and the drop is counted (`medulla_observe_dropped_total`).
//! A name longer than [`MAX_TASK_NAME_BYTES`] is cut at a character boundary before it is
//! listed. The last [`FINISHED_KEPT`] finished rows stay readable through
//! [`TaskRegistry::get`].

use std::collections::{HashMap, VecDeque};
use std::panic::Location;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use medulla_types::transition_refused::TransitionRefused;
use parking_lot::Mutex;

use crate::machine::task::{TaskEvent, TaskFailure, TaskState};

/// The live-entry capacity of the process-wide registry (`observe.task_registry_capacity`).
pub const DEFAULT_TASK_REGISTRY_CAPACITY: usize = 4096;
/// The longest task name, in bytes.
pub const MAX_TASK_NAME_BYTES: usize = 128;
/// The longest `ProgressCursor::Opaque` value, in bytes.
pub const MAX_OPAQUE_CURSOR_BYTES: usize = 256;
/// How many finished rows the registry keeps for reading after the job ended.
pub const FINISHED_KEPT: usize = 256;

/// What a registered job is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TaskKind {
    /// Paging a graph into memory.
    Materialization,
    /// Building an index.
    Index,
    /// Migrating a store or a namespace.
    Migration,
    /// Periodic housekeeping.
    Maintenance,
    /// A projection drain.
    Projection,
    /// Replica pull.
    Replication,
    /// An accept loop.
    Listener,
    /// A job a consumer outside the closed inventory registered.
    External,
}

/// What `register` takes.
#[derive(Clone, Debug)]
pub struct TaskSpec {
    /// Unique among live tasks; at most [`MAX_TASK_NAME_BYTES`] bytes.
    pub name: String,
    /// The subsystem that spawned the job.
    pub owner: String,
    /// What the job is.
    pub kind: TaskKind,
}

/// Where a job is in its work. Never a payload or a secret.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProgressCursor {
    /// A keyset position: the last node key and the last `(source, target, ordinal)` edge key.
    Keyset {
        /// The last node key paged.
        node: Option<String>,
        /// The last edge key paged.
        edge: Option<(String, String, u32)>,
    },
    /// A monotonic counter.
    Counter(u64),
    /// An opaque position of at most [`MAX_OPAQUE_CURSOR_BYTES`] bytes.
    Opaque(String),
}

impl ProgressCursor {
    /// An opaque cursor, cut at a character boundary to [`MAX_OPAQUE_CURSOR_BYTES`].
    pub fn opaque(value: impl Into<String>) -> Self {
        let mut value = value.into();
        cut_at_boundary(&mut value, MAX_OPAQUE_CURSOR_BYTES);
        Self::Opaque(value)
    }
}

fn cut_at_boundary(value: &mut String, limit: usize) {
    if value.len() <= limit {
        return;
    }
    let mut end = limit;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value.truncate(end);
}

/// How a job ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The job finished.
    Completed,
    /// The job failed.
    Failed(TaskFailure),
    /// The job was cancelled.
    Cancelled,
}

/// One registered job as the introspection reads it.
#[derive(Clone, Debug)]
pub struct TaskRow {
    /// The registry's sequence number of this registration.
    pub id: u64,
    /// The job's unique name.
    pub name: String,
    /// The subsystem that spawned it.
    pub owner: String,
    /// What it is.
    pub kind: TaskKind,
    /// The state of `TaskMachine`.
    pub state: TaskState,
    /// Set when `state` is `Failed`.
    pub failure: Option<TaskFailure>,
    /// The cursor of the last `progress` call.
    pub cursor: Option<ProgressCursor>,
    /// When the last `progress` call ran; 0 before the first.
    pub last_progress_unix_ms: u64,
    /// The caller of the last `progress` call.
    pub last_progress_location: Option<&'static Location<'static>>,
    /// The caller of `register`.
    pub registered_location: &'static Location<'static>,
    /// Units of work done; never decreases.
    pub units_done: u64,
    /// The request that spawned the job.
    pub origin_request: Option<u64>,
}

/// `file:line` of a recorded call site.
pub fn file_line(location: &Location<'_>) -> String {
    format!("{}:{}", location.file(), location.line())
}

type Cell = Arc<Mutex<TaskRow>>;

#[derive(Default)]
struct Live {
    by_name: HashMap<String, Cell>,
    finished: VecDeque<TaskRow>,
}

struct Inner {
    capacity: usize,
    next_id: AtomicU64,
    dropped: AtomicU64,
    live: Mutex<Live>,
}

/// The registry of background jobs. Cloning shares the registry.
#[derive(Clone)]
pub struct TaskRegistry {
    inner: Arc<Inner>,
}

impl TaskRegistry {
    /// An isolated registry holding at most `capacity` live entries.
    pub fn new(capacity: usize) -> Self {
        Self {
            inner: Arc::new(Inner {
                capacity,
                next_id: AtomicU64::new(1),
                dropped: AtomicU64::new(0),
                live: Mutex::new(Live::default()),
            }),
        }
    }

    /// Jobs this registry could not list since it was built.
    pub fn dropped_total(&self) -> u64 {
        self.inner.dropped.load(Ordering::Relaxed)
    }

    /// Register a job. The state is `Registered`; the first `progress` or `finish` starts it.
    #[track_caller]
    pub fn register(&self, spec: TaskSpec) -> TaskHandle {
        let registered_location = Location::caller();
        let mut name = spec.name;
        cut_at_boundary(&mut name, MAX_TASK_NAME_BYTES);
        let row = TaskRow {
            id: self.inner.next_id.fetch_add(1, Ordering::Relaxed),
            name: name.clone(),
            owner: spec.owner,
            kind: spec.kind,
            state: TaskState::initial(),
            failure: None,
            cursor: None,
            last_progress_unix_ms: 0,
            last_progress_location: None,
            registered_location,
            units_done: 0,
            origin_request: None,
        };
        let cell: Cell = Arc::new(Mutex::new(row));
        let listed = self.list(name, &cell);
        TaskHandle {
            registry: self.clone(),
            progress: TaskProgress { cell },
            listed,
            ended: false,
        }
    }

    fn list(&self, name: String, cell: &Cell) -> bool {
        let mut live = self.inner.live.lock();
        if live.by_name.len() >= self.inner.capacity || live.by_name.contains_key(&name) {
            drop(live);
            self.inner.dropped.fetch_add(1, Ordering::Relaxed);
            tracing::warn!(
                target: "medulla::observe",
                task = %name,
                "task not listed: the registry is full or the name is held by a live task"
            );
            return false;
        }
        live.by_name.insert(name, Arc::clone(cell));
        true
    }

    /// The live row named `name`, else the most recent finished row of that name.
    pub fn get(&self, name: &str) -> Option<TaskRow> {
        let live = self.inner.live.lock();
        if let Some(cell) = live.by_name.get(name) {
            return Some(cell.lock().clone());
        }
        live.finished.iter().rev().find(|row| row.name == name).cloned()
    }

    /// The live rows, ordered by registration.
    pub fn live(&self) -> Vec<TaskRow> {
        let live = self.inner.live.lock();
        let mut rows: Vec<TaskRow> = live.by_name.values().map(|cell| cell.lock().clone()).collect();
        rows.sort_by_key(|row| row.id);
        rows
    }

    fn end(&self, cell: &Cell, listed: bool) {
        let row = cell.lock().clone();
        let mut live = self.inner.live.lock();
        if listed {
            live.by_name.remove(&row.name);
            if live.finished.len() >= FINISHED_KEPT {
                live.finished.pop_front();
            }
            live.finished.push_back(row);
        }
    }
}

/// The one writer of a task's state: the next state comes from `TaskState::apply`.
fn apply_event(row: &mut TaskRow, event: TaskEvent) -> Result<(), TransitionRefused> {
    row.state = row.state.apply(event)?;
    Ok(())
}

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as u64)
}

fn record_progress(
    cell: &Cell,
    cursor: ProgressCursor,
    units_done: u64,
    location: &'static Location<'static>,
) {
    let now = unix_ms();
    let mut row = cell.lock();
    let started = if row.state == TaskState::Registered {
        apply_event(&mut row, TaskEvent::Start)
    } else {
        Ok(())
    };
    if let Err(refused) = started.and_then(|()| apply_event(&mut row, TaskEvent::Progress)) {
        tracing::warn!(target: "medulla::transition", task = %row.name, "{refused}");
        return;
    }
    row.cursor = Some(match cursor {
        ProgressCursor::Opaque(value) => ProgressCursor::opaque(value),
        other => other,
    });
    row.units_done = row.units_done.max(units_done);
    row.last_progress_unix_ms = now;
    row.last_progress_location = Some(location);
}

/// A cloneable reporter of one task's progress. [`crate::spawn::progress`] reaches the current
/// task's reporter without a handle in scope.
#[derive(Clone)]
pub struct TaskProgress {
    cell: Cell,
}

impl TaskProgress {
    /// Record the cursor, the units done and the caller location.
    #[track_caller]
    pub fn progress(&self, cursor: ProgressCursor, units_done: u64) {
        record_progress(&self.cell, cursor, units_done, Location::caller());
    }
}

/// The handle of one registered job. Dropped without `finish`, it records `Failed(TASK_DROPPED)`.
pub struct TaskHandle {
    registry: TaskRegistry,
    progress: TaskProgress,
    listed: bool,
    ended: bool,
}

impl TaskHandle {
    /// Record the cursor, the units done and the caller location. The first call starts the task.
    #[track_caller]
    pub fn progress(&self, cursor: ProgressCursor, units_done: u64) {
        record_progress(&self.progress.cell, cursor, units_done, Location::caller());
    }

    /// A cloneable reporter of this task's progress.
    pub fn reporter(&self) -> TaskProgress {
        self.progress.clone()
    }

    /// The current row of this task, listed or not.
    pub fn row(&self) -> TaskRow {
        self.progress.cell.lock().clone()
    }

    /// End the job with `outcome`.
    pub fn finish(mut self, outcome: Outcome) {
        self.end(outcome);
    }

    fn end(&mut self, outcome: Outcome) {
        if self.ended {
            return;
        }
        self.ended = true;
        {
            let mut row = self.progress.cell.lock();
            let result = match outcome {
                Outcome::Completed => {
                    let started = if row.state == TaskState::Registered {
                        apply_event(&mut row, TaskEvent::Start)
                    } else {
                        Ok(())
                    };
                    started.and_then(|()| apply_event(&mut row, TaskEvent::Finish))
                }
                Outcome::Failed(failure) => {
                    let applied = apply_event(&mut row, TaskEvent::Fail);
                    if applied.is_ok() {
                        row.failure = Some(failure);
                    }
                    applied
                }
                Outcome::Cancelled => apply_event(&mut row, TaskEvent::Cancel),
            };
            if let Err(refused) = result {
                tracing::warn!(target: "medulla::transition", task = %row.name, "{refused}");
            }
        }
        self.registry.end(&self.progress.cell, self.listed);
    }
}

impl Drop for TaskHandle {
    fn drop(&mut self) {
        self.end(Outcome::Failed(TaskFailure::Dropped));
    }
}
