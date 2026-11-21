//! The `spawn` helper (MEDULLA-OBSERVE-R003.2).
//!
//! [`spawn`] registers a job in the process-wide registry, enters the span current at the
//! call so the job keeps the request's trace fields, runs the future and finishes the handle
//! with the future's outcome ([`IntoOutcome`]). A panic finishes the handle with
//! `Failed(TASK_PANICKED)` and the panic location. Inside the job, [`progress`] reports to the
//! job's handle without the handle in scope.
//!
//! This is the one place the engine calls `tokio::spawn` for a registered background job.

use std::cell::RefCell;
use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::sync::Once;

use futures_util::FutureExt;
use medulla_types::transition_refused::TransitionRefused;
use tracing::Instrument;

use crate::machine::task::TaskFailure;
use crate::registry::tasks::{Outcome, ProgressCursor, TaskProgress, TaskSpec};

tokio::task_local! {
    static CURRENT: TaskProgress;
}

thread_local! {
    static LAST_PANIC: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// A refusal text that names a typed code: the code a failed job records.
pub trait FailureCode {
    /// The typed refusal code of this error.
    fn failure_code(&self) -> String;
}

impl FailureCode for String {
    /// The leading `CODE` of a `CODE: detail` text, else `INTERNAL`.
    fn failure_code(&self) -> String {
        match self.split_once(':') {
            Some((code, _))
                if !code.is_empty()
                    && code
                        .bytes()
                        .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_') =>
            {
                code.to_string()
            }
            _ => "INTERNAL".to_string(),
        }
    }
}

impl FailureCode for TransitionRefused {
    fn failure_code(&self) -> String {
        self.code().as_str().to_string()
    }
}

/// How a job's future ends in the registry.
pub trait IntoOutcome {
    /// The outcome the handle finishes with.
    fn into_outcome(self) -> Outcome;
}

impl IntoOutcome for () {
    fn into_outcome(self) -> Outcome {
        Outcome::Completed
    }
}

impl IntoOutcome for Outcome {
    fn into_outcome(self) -> Outcome {
        self
    }
}

impl<T, E: FailureCode> IntoOutcome for Result<T, E> {
    fn into_outcome(self) -> Outcome {
        match self {
            Ok(_) => Outcome::Completed,
            Err(error) => Outcome::Failed(TaskFailure::Refused {
                code: error.failure_code(),
            }),
        }
    }
}

fn install_panic_hook() {
    static HOOK: Once = Once::new();
    HOOK.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            if let Some(location) = info.location() {
                let text = format!("{}:{}", location.file(), location.line());
                LAST_PANIC.with(|cell| *cell.borrow_mut() = Some(text));
            }
            previous(info);
        }));
    });
}

fn take_panic_location() -> String {
    LAST_PANIC
        .with(|cell| cell.borrow_mut().take())
        .unwrap_or_else(|| "unknown".to_string())
}

/// Register the job `spec`, run `future` as a task and finish the handle with its outcome.
///
/// The returned handle completes when the job has finished and its row is final.
#[track_caller]
pub fn spawn<F>(spec: TaskSpec, future: F) -> tokio::task::JoinHandle<()>
where
    F: Future + Send + 'static,
    F::Output: IntoOutcome,
{
    install_panic_hook();
    let handle = crate::global().register(spec);
    let reporter = handle.reporter();
    let span = tracing::Span::current();
    // The one bare spawn for a registered job: every other spawn site calls this helper.
    tokio::spawn(async move {
        let result = CURRENT
            .scope(
                reporter,
                AssertUnwindSafe(future.instrument(span)).catch_unwind(),
            )
            .await;
        let outcome = match result {
            Ok(output) => output.into_outcome(),
            Err(_) => Outcome::Failed(TaskFailure::Panicked {
                location: take_panic_location(),
            }),
        };
        handle.finish(outcome);
    })
}

/// Report progress of the job this call runs in: the cursor, the units done and the caller
/// location. Returns `false` outside a job started by [`spawn`].
#[track_caller]
pub fn progress(cursor: ProgressCursor, units_done: u64) -> bool {
    let Ok(reporter) = CURRENT.try_with(TaskProgress::clone) else {
        return false;
    };
    reporter.progress(cursor, units_done);
    true
}
