//! The observability knobs the registries read after the server loaded them
//! (MEDULLA-OBSERVE-R011).
//!
//! The knob table lives in `medulla-server-api`, which sits above this crate, so the server hands
//! each resolved value down once at start through the setters here. Each setting starts at the
//! default of its knob row, so a registry built before the server loaded its knobs, and every
//! test, runs with the documented defaults.

use std::sync::atomic::{AtomicU64, Ordering};

/// The default `observe.task_stall_after_ms`.
pub const DEFAULT_TASK_STALL_AFTER_MS: u64 = 60_000;
/// The default `observe.stall_check_interval_ms`.
pub const DEFAULT_STALL_CHECK_INTERVAL_MS: u64 = 5_000;
/// The default `observe.health_deadline_ms`.
pub const DEFAULT_HEALTH_DEADLINE_MS: u64 = 2_000;
/// The default `observe.startup_reports_kept`.
pub const DEFAULT_STARTUP_REPORTS_KEPT: u64 = 8;
/// The default `observe.profile_hz`.
pub const DEFAULT_PROFILE_HZ: u64 = 99;
/// The value of `observe.profile_max_seconds`, a fixed knob.
pub const PROFILE_MAX_SECONDS: u64 = 60;

static TASK_STALL_AFTER_MS: AtomicU64 = AtomicU64::new(DEFAULT_TASK_STALL_AFTER_MS);
static STALL_CHECK_INTERVAL_MS: AtomicU64 = AtomicU64::new(DEFAULT_STALL_CHECK_INTERVAL_MS);
static HEALTH_DEADLINE_MS: AtomicU64 = AtomicU64::new(DEFAULT_HEALTH_DEADLINE_MS);
static STARTUP_REPORTS_KEPT: AtomicU64 = AtomicU64::new(DEFAULT_STARTUP_REPORTS_KEPT);
static PROFILE_HZ: AtomicU64 = AtomicU64::new(DEFAULT_PROFILE_HZ);

/// Set `observe.task_stall_after_ms`: how old a task's last progress may be before the watchdog
/// marks it stalled.
pub fn set_task_stall_after_ms(ms: u64) {
    TASK_STALL_AFTER_MS.store(ms, Ordering::Relaxed);
}

/// `observe.task_stall_after_ms`.
pub fn task_stall_after_ms() -> u64 {
    TASK_STALL_AFTER_MS.load(Ordering::Relaxed)
}

/// Set `observe.stall_check_interval_ms`: how often the watchdog scans the live tasks.
pub fn set_stall_check_interval_ms(ms: u64) {
    STALL_CHECK_INTERVAL_MS.store(ms.max(1), Ordering::Relaxed);
}

/// `observe.stall_check_interval_ms`.
pub fn stall_check_interval_ms() -> u64 {
    STALL_CHECK_INTERVAL_MS.load(Ordering::Relaxed)
}

/// Set `observe.health_deadline_ms`: the longest `Health` and `/readyz` wait for the state.
pub fn set_health_deadline_ms(ms: u64) {
    HEALTH_DEADLINE_MS.store(ms, Ordering::Relaxed);
}

/// `observe.health_deadline_ms`.
pub fn health_deadline_ms() -> u64 {
    HEALTH_DEADLINE_MS.load(Ordering::Relaxed)
}

/// Set `observe.startup_reports_kept`: how many boot reports persist.
pub fn set_startup_reports_kept(count: u64) {
    STARTUP_REPORTS_KEPT.store(count.max(1), Ordering::Relaxed);
}

/// `observe.startup_reports_kept`.
pub fn startup_reports_kept() -> u64 {
    STARTUP_REPORTS_KEPT.load(Ordering::Relaxed)
}

/// Set `observe.profile_hz`: the stack sampling rate.
pub fn set_profile_hz(hz: u64) {
    PROFILE_HZ.store(hz.max(1), Ordering::Relaxed);
}

/// `observe.profile_hz`.
pub fn profile_hz() -> u64 {
    PROFILE_HZ.load(Ordering::Relaxed)
}
