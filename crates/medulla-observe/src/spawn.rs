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

use crate::machine::task::{TaskFailure, NO_PROGRESS};
use crate::registry::tasks::{Outcome, ProgressCursor, TaskProgress, TaskSpec};

tokio::task_local! {
    static CURRENT: TaskProgress;
    static CURRENT_NAME: String;
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
            Err(error) => {
                let code = error.failure_code();
                Outcome::Failed(if code == NO_PROGRESS {
                    TaskFailure::NoProgress
                } else {
                    TaskFailure::Refused { code }
                })
            }
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

/// What a job inherits from the code that spawns it (MEDULLA-OBSERVE-R002.4): the span current
/// at the spawn, which carries the request id and the trace id of the request that spawned it,
/// and that request's id. A job that outlives its request keeps both.
#[derive(Clone, Debug)]
pub struct SpawnContext {
    /// The span current at the spawn.
    pub span: tracing::Span,
    /// The id of the request the spawn ran under, when it ran under one.
    pub origin_request: Option<u64>,
}

impl SpawnContext {
    /// Capture the context of the calling code: one `Arc` clone inside the span and one
    /// task-local read.
    pub fn capture() -> Self {
        Self {
            span: tracing::Span::current(),
            origin_request: crate::registry::requests::current()
                .map(|request| request.request_id()),
        }
    }
}

/// Register the job `spec`, run `future` as a task and finish the handle with its outcome.
///
/// The job runs inside the span current at the call, so its log lines carry the request's
/// ids, and its row records the request that spawned it as `origin_request`.
///
/// The returned handle completes when the job has finished and its row is final.
#[track_caller]
pub fn spawn<F>(spec: TaskSpec, future: F) -> tokio::task::JoinHandle<()>
where
    F: Future + Send + 'static,
    F::Output: IntoOutcome,
{
    install_panic_hook();
    let name = spec.name.clone();
    let handle = crate::global().register(spec);
    let reporter = handle.reporter();
    let context = SpawnContext::capture();
    handle.set_origin_request(context.origin_request);
    // The one bare spawn for a registered job: every other spawn site calls this helper. The
    // whole job, its end included, runs inside the inherited span.
    tokio::spawn(
        async move {
            let result = CURRENT_NAME
                .scope(
                    name,
                    CURRENT.scope(reporter, AssertUnwindSafe(future).catch_unwind()),
                )
                .await;
            let outcome = match result {
                Ok(output) => output.into_outcome(),
                Err(_) => Outcome::Failed(TaskFailure::Panicked {
                    location: take_panic_location(),
                }),
            };
            handle.finish(outcome);
        }
        .instrument(context.span),
    )
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

/// A cloneable reporter of the job this call runs in, or `None` outside a job started by
/// [`spawn`]. Code that hands its progress to a blocking thread takes the reporter first: a
/// task-local does not follow the work onto another thread.
pub fn current_reporter() -> Option<TaskProgress> {
    CURRENT.try_with(TaskProgress::clone).ok()
}

/// The registered name of the job this call runs in, or `None` outside a job started by
/// [`spawn`]. Lock sites use it to name a holder that is not a request (MEDULLA-OBSERVE-R006.4).
pub fn current_task_name() -> Option<String> {
    CURRENT_NAME.try_with(String::clone).ok()
}
