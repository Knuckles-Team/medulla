//! Foreground activity and the latest downtime selections
//! (MEDULLA-DURABLE-KERNEL-R101.3).
//!
//! Every served request calls [`FOREGROUND_ACTIVITY`]`.note_request`; the downtime
//! tick reads it to decide whether the idle window is open. The tick also records
//! what it selected here so the served `DowntimeSelections` method can answer it
//! without reaching into the server crate.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

pub use medulla_work_types::downtime::{DowntimeSelected, DowntimeSelections};

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

static LAST_SELECTIONS: Mutex<Option<DowntimeSelections>> = Mutex::new(None);

/// Record the latest tick's decision.
pub fn record_selections(selections: DowntimeSelections) {
    if let Ok(mut slot) = LAST_SELECTIONS.lock() {
        *slot = Some(selections);
    }
}

/// The latest tick's decision, or an empty one before the first tick.
pub fn latest_selections() -> DowntimeSelections {
    LAST_SELECTIONS
        .lock()
        .ok()
        .and_then(|slot| slot.clone())
        .unwrap_or_default()
}
