//! Foreground activity and the downtime job bodies (MEDULLA-DURABLE-KERNEL-R101.3, R101.7).
//!
//! Every served request calls [`FOREGROUND_ACTIVITY`]`.note_request`; the downtime
//! tick reads it to decide whether the idle window is open, and the downtime worker
//! reads it to decide whether to yield. The tick's selections are graph facts in the
//! commons graph (R101.5), not state held here.

#[cfg(feature = "jobs")]
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
#[cfg(feature = "jobs")]
use std::sync::{Arc, RwLock};

/// A request-rate tracker: one atomic second counter and the last busy instant.
#[derive(Debug)]
pub struct ForegroundActivity {
    /// The wall-clock second the counter below belongs to.
    second: AtomicU64,
    /// Requests seen in `second`.
    count: AtomicU64,
    /// Millisecond of the last second whose request count passed the idle ceiling.
    last_busy_ms: AtomicU64,
}

/// One reading of [`ForegroundActivity`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ActivityReading {
    /// Requests per second over the current second.
    pub peak_rps: f64,
    /// Whole seconds since the request rate last passed the idle ceiling.
    pub quiet_secs: u32,
}

impl ForegroundActivity {
    /// An empty tracker: no request seen, quiet since the epoch.
    pub const fn new() -> Self {
        Self {
            second: AtomicU64::new(0),
            count: AtomicU64::new(0),
            last_busy_ms: AtomicU64::new(0),
        }
    }

    /// Record one served request at `now_ms` against the idle ceiling
    /// `idle_max_rps`. A second whose count passes the ceiling marks the
    /// foreground busy at that instant.
    pub fn note_request(&self, now_ms: u64, idle_max_rps: u32) {
        let second = now_ms / 1000;
        if self.second.swap(second, Ordering::Relaxed) != second {
            self.count.store(0, Ordering::Relaxed);
        }
        let seen = self.count.fetch_add(1, Ordering::Relaxed) + 1;
        if seen > u64::from(idle_max_rps) {
            self.last_busy_ms.fetch_max(now_ms, Ordering::Relaxed);
        }
    }

    /// Read the tracker at `now_ms`.
    pub fn reading(&self, now_ms: u64) -> ActivityReading {
        let in_current_second = self.second.load(Ordering::Relaxed) == now_ms / 1000;
        let peak = if in_current_second {
            self.count.load(Ordering::Relaxed)
        } else {
            0
        };
        let quiet_ms = now_ms.saturating_sub(self.last_busy_ms.load(Ordering::Relaxed));
        ActivityReading {
            peak_rps: peak as f64,
            quiet_secs: u32::try_from(quiet_ms / 1000).unwrap_or(u32::MAX),
        }
    }
}

impl Default for ForegroundActivity {
    fn default() -> Self {
        Self::new()
    }
}

/// The process-wide foreground-activity tracker.
pub static FOREGROUND_ACTIVITY: ForegroundActivity = ForegroundActivity::new();

/// What one step of a downtime job needs: the claimed job, the state its previous step
/// checkpointed and the wall-clock time the step may use (MEDULLA-DURABLE-KERNEL-R101.7).
#[cfg(feature = "jobs")]
pub struct DowntimeStepInput<'a> {
    /// The claimed job.
    pub job: &'a medulla_jobs::AnalyticsJob,
    /// The `state_blob` of the job's last checkpoint; `None` on a first step.
    pub state_blob: Option<&'a [u8]>,
    /// The longest the step may run, in milliseconds.
    pub step_deadline_ms: u64,
}

/// What one step of a downtime job answers.
#[cfg(feature = "jobs")]
pub enum DowntimeStep {
    /// More work remains: the state the next step resumes from and the bytes the step held.
    Continue {
        /// The state the worker checkpoints.
        state_blob: Vec<u8>,
        /// The working memory the step held, in bytes.
        working_bytes: u64,
    },
    /// The job is finished: its typed result and the bytes the last step held.
    Done {
        /// The job's typed result.
        result: medulla_jobs::TypedJobResult,
        /// The working memory the step held, in bytes.
        working_bytes: u64,
    },
}

/// The work of one downtime intent, run in bounded steps by the downtime worker.
#[cfg(feature = "jobs")]
pub trait DowntimeBody: Send + Sync {
    /// Run one step. An error fails the attempt.
    fn step(&self, input: DowntimeStepInput<'_>) -> Result<DowntimeStep, String>;
}

#[cfg(feature = "jobs")]
static BODIES: RwLock<BTreeMap<&'static str, Arc<dyn DowntimeBody>>> = RwLock::new(BTreeMap::new());

/// Install the body of `intent`; a second install for the same intent replaces the first.
#[cfg(feature = "jobs")]
pub fn install_downtime_body(intent: &'static str, body: Arc<dyn DowntimeBody>) {
    if let Ok(mut bodies) = BODIES.write() {
        bodies.insert(intent, body);
    }
}

/// The body installed for `intent`, if any.
#[cfg(feature = "jobs")]
pub fn downtime_body(intent: &str) -> Option<Arc<dyn DowntimeBody>> {
    BODIES.read().ok()?.get(intent).cloned()
}

/// The intents that have a body installed, in name order.
#[cfg(feature = "jobs")]
pub fn downtime_body_intents() -> Vec<&'static str> {
    BODIES
        .read()
        .map(|bodies| bodies.keys().copied().collect())
        .unwrap_or_default()
}
