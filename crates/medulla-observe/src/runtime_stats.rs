//! Runtime worker statistics (MEDULLA-OBSERVE-R006.3).
//!
//! [`RuntimeSampler::snapshot`] reads `Handle::metrics()` and reports, per tokio worker, the
//! busy duration, the busy ratio over the last window and the park count, plus the global
//! queue depth and the worker count. A worker whose busy ratio is at
//! least the spin threshold (`observe.worker_spin_busy_ratio`) across two consecutive windows
//! is flagged `spinning`.
//!
//! A window is the time between two roll points: a snapshot rolls the window when at least
//! the window length has passed since the last roll, so the ratio is exact however often the
//! section is read. Only metrics tokio stabilizes are read: the worker count, the global queue
//! depth, each worker's total busy duration and park count. Metrics that need
//! `tokio_unstable` (local queue depth, blocking-pool counts) are not reported at all.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use parking_lot::Mutex;

/// The default window length.
pub const DEFAULT_WINDOW: Duration = Duration::from_secs(1);
/// The default `observe.worker_spin_busy_ratio`.
pub const DEFAULT_SPIN_BUSY_RATIO: f64 = 0.9;

/// One worker's row.
#[derive(Clone, Debug, PartialEq)]
pub struct WorkerRow {
    /// The worker's index.
    pub index: usize,
    /// Total time the worker spent running tasks, in milliseconds.
    pub busy_ms: u64,
    /// The fraction of the last window the worker was busy, in `0.0..=1.0`.
    pub busy_ratio: f64,
    /// How many times the worker parked.
    pub parks: u64,
    /// Busy at or above the spin threshold across two windows.
    pub spinning: bool,
}

/// A reading of the whole runtime.
#[derive(Clone, Debug, PartialEq)]
pub struct RuntimeSnapshot {
    /// One row per worker, ascending by index.
    pub workers: Vec<WorkerRow>,
    /// Tasks in the global queue.
    pub global_queue_depth: usize,
}

struct State {
    started: Instant,
    busy_at_start: Vec<Duration>,
    last: Option<Vec<f64>>,
    previous: Option<Vec<f64>>,
}

/// Samples the runtime and keeps the window state the ratios need.
pub struct RuntimeSampler {
    window: Duration,
    spin_ratio_bits: AtomicU64,
    state: Mutex<Option<State>>,
}

impl Default for RuntimeSampler {
    fn default() -> Self {
        Self::new()
    }
}

impl RuntimeSampler {
    /// A sampler with the default window and spin threshold.
    pub fn new() -> Self {
        Self::with_window(DEFAULT_WINDOW)
    }

    /// A sampler whose window is `window`.
    pub fn with_window(window: Duration) -> Self {
        Self {
            window,
            spin_ratio_bits: AtomicU64::new(DEFAULT_SPIN_BUSY_RATIO.to_bits()),
            state: Mutex::new(None),
        }
    }

    /// Set `observe.worker_spin_busy_ratio`.
    pub fn set_spin_busy_ratio(&self, ratio: f64) {
        self.spin_ratio_bits.store(ratio.to_bits(), Ordering::Relaxed);
    }

    fn spin_busy_ratio(&self) -> f64 {
        f64::from_bits(self.spin_ratio_bits.load(Ordering::Relaxed))
    }

    /// Read the runtime behind `handle`.
    pub fn snapshot(&self, handle: &tokio::runtime::Handle) -> RuntimeSnapshot {
        let metrics = handle.metrics();
        let workers = metrics.num_workers();
        let busy: Vec<Duration> = (0..workers)
            .map(|index| metrics.worker_total_busy_duration(index))
            .collect();
        let now = Instant::now();
        let spin = self.spin_busy_ratio();
        let mut guard = self.state.lock();
        let state = guard.get_or_insert_with(|| State {
            started: now,
            busy_at_start: busy.clone(),
            last: None,
            previous: None,
        });
        if state.busy_at_start.len() != workers {
            *state = State {
                started: now,
                busy_at_start: busy.clone(),
                last: None,
                previous: None,
            };
        }
        let elapsed = now.duration_since(state.started);
        let ratios_over = |state: &State| -> Vec<f64> {
            let wall = elapsed.as_secs_f64();
            busy.iter()
                .zip(&state.busy_at_start)
                .map(|(now_busy, start_busy)| {
                    if wall <= 0.0 {
                        0.0
                    } else {
                        (now_busy.saturating_sub(*start_busy).as_secs_f64() / wall).clamp(0.0, 1.0)
                    }
                })
                .collect()
        };
        if elapsed >= self.window {
            let ratios = ratios_over(state);
            state.previous = state.last.take();
            state.last = Some(ratios);
            state.started = now;
            state.busy_at_start = busy.clone();
        }
        let reported = match &state.last {
            Some(last) => last.clone(),
            None => ratios_over(state),
        };
        let rows = (0..workers)
            .map(|index| {
                let ratio = reported[index];
                let spinning = match (&state.last, &state.previous) {
                    (Some(last), Some(previous)) => last[index] >= spin && previous[index] >= spin,
                    _ => false,
                };
                WorkerRow {
                    index,
                    busy_ms: busy[index].as_millis() as u64,
                    busy_ratio: ratio,
                    parks: metrics.worker_park_count(index),
                    spinning,
                }
            })
            .collect();
        RuntimeSnapshot {
            workers: rows,
            global_queue_depth: metrics.global_queue_depth(),
        }
    }
}
