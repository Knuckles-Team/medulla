//! The progress guard (MEDULLA-OBSERVE-R005.1).
//!
//! A loop that must move creates a [`ProgressGuard`] before it starts and calls
//! [`ProgressGuard::check`] with its cursor after each pass. A cursor that differs from the
//! last one is progress. After `observe.progress_idle_limit` equal cursors in a row `check`
//! returns [`NoProgress`], which names the loop and the `file:line` that created the guard and
//! maps to the code `NO_PROGRESS`.

use std::fmt;
use std::panic::Location;
use std::sync::atomic::{AtomicU32, Ordering};

use medulla_types::contract::ServerErrorCode;

use crate::registry::tasks::{file_line, ProgressCursor};

/// The default `observe.progress_idle_limit`.
pub const DEFAULT_PROGRESS_IDLE_LIMIT: u32 = 2;

static IDLE_LIMIT: AtomicU32 = AtomicU32::new(DEFAULT_PROGRESS_IDLE_LIMIT);

/// Set the idle limit every guard created afterwards starts with (`observe.progress_idle_limit`,
/// read once at start). A limit of zero is raised to one.
pub fn set_progress_idle_limit(limit: u32) {
    IDLE_LIMIT.store(limit.max(1), Ordering::Relaxed);
}

/// The idle limit a new guard starts with.
pub fn progress_idle_limit() -> u32 {
    IDLE_LIMIT.load(Ordering::Relaxed)
}

/// A loop made no progress.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NoProgress {
    /// The loop's name.
    pub name: String,
    /// The caller of [`ProgressGuard::new`].
    pub location: &'static Location<'static>,
    /// The cursor that repeated.
    pub cursor: ProgressCursor,
    /// How many equal cursors in a row the guard saw.
    pub idle_iterations: u32,
}

impl NoProgress {
    /// The registered refusal code of this error.
    pub fn code(&self) -> ServerErrorCode {
        ServerErrorCode::NoProgress
    }
}

impl fmt::Display for NoProgress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}: loop '{}' at {} made no progress; cursor {:?} repeated {} times in a row",
            self.code().as_str(),
            self.name,
            file_line(self.location),
            self.cursor,
            self.idle_iterations
        )
    }
}

impl std::error::Error for NoProgress {}

/// Fails a loop whose cursor stops moving.
#[derive(Debug)]
pub struct ProgressGuard {
    name: String,
    location: &'static Location<'static>,
    limit: u32,
    last: Option<ProgressCursor>,
    idle: u32,
}

impl ProgressGuard {
    /// A guard for the loop `name`, recording the call site. The idle limit is
    /// [`progress_idle_limit`] at this call.
    #[track_caller]
    pub fn new(name: impl Into<String>) -> Self {
        Self::with_limit(name, progress_idle_limit())
    }

    /// A guard with an explicit idle limit.
    #[track_caller]
    pub fn with_limit(name: impl Into<String>, limit: u32) -> Self {
        Self {
            name: name.into(),
            location: Location::caller(),
            limit: limit.max(1),
            last: None,
            idle: 0,
        }
    }

    /// `Ok` when `cursor` differs from the last one. After the idle limit of equal cursors in
    /// a row, [`NoProgress`].
    pub fn check(&mut self, cursor: &ProgressCursor) -> Result<(), NoProgress> {
        if self.last.as_ref() != Some(cursor) {
            self.last = Some(cursor.clone());
            self.idle = 0;
            return Ok(());
        }
        self.idle += 1;
        if self.idle < self.limit {
            return Ok(());
        }
        Err(NoProgress {
            name: self.name.clone(),
            location: self.location,
            cursor: cursor.clone(),
            idle_iterations: self.idle,
        })
    }
}
