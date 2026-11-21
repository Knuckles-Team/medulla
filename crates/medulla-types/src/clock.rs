//! The injected time source: one trait for every component that decides on
//! the current time, so a test drives a [`ManualClock`] instead of waiting on
//! the wall clock.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// The time source of a component, in milliseconds.
pub trait Clock: Send + Sync {
    /// The current time.
    fn now_ms(&self) -> u64;
}

/// The wall clock: milliseconds since the Unix epoch.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_millis() as u64)
            .unwrap_or(0)
    }
}

/// A clock that moves only when told to. Clones share one time.
#[derive(Clone, Debug, Default)]
pub struct ManualClock(Arc<AtomicU64>);

impl ManualClock {
    /// A clock reading `start_ms`.
    pub fn new(start_ms: u64) -> Self {
        Self(Arc::new(AtomicU64::new(start_ms)))
    }

    /// Move the clock forward by `ms`.
    pub fn advance_ms(&self, ms: u64) {
        self.0.fetch_add(ms, Ordering::Relaxed);
    }
}

impl Clock for ManualClock {
    fn now_ms(&self) -> u64 {
        self.0.load(Ordering::Relaxed)
    }
}
