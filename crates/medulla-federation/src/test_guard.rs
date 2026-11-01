//! Test support: the process-global loopback allowlist guard for mock HTTP federation sources.

use std::ffi::OsString;

use parking_lot::{Mutex, MutexGuard};

/// Non-poisoning on purpose: it guards `()`, so a test that panicked while holding
/// it leaves no state behind to distrust, and std's poison would carry no information.
static FEDERATION_ALLOW_ENV_LOCK: Mutex<()> = Mutex::new(());

/// Serialize the process-global test override and restore the caller's environment.
/// The value is the exact origin, so this does not weaken the production SSRF default.
///
/// `pub(crate)`: also used by `advanced_crossmodal_tests`'s `federation_sparql_local_
/// vector_foreign_sql_one_plan_eg385`, the other test-only mock-HTTP federation source
/// in this crate, for the identical reason.
/// Holds the loopback allowlist override for one test; restores the caller's environment on drop.
pub struct MockHttpAllowGuard {
    _lock: MutexGuard<'static, ()>,
    previous: Option<OsString>,
}

impl MockHttpAllowGuard {
    /// Allow exactly this mock server origin until the guard drops.
    pub fn new(url: &str) -> Self {
        let lock = FEDERATION_ALLOW_ENV_LOCK.lock();
        let previous = std::env::var_os(crate::federation::HTTP_JSON_FEDERATION_ALLOW_ENV);
        let origin = url.trim_end_matches('/');
        let configured = previous
            .as_ref()
            .map(|value| format!("{},{origin}", value.to_string_lossy()))
            .unwrap_or_else(|| origin.to_string());
        std::env::set_var(
            crate::federation::HTTP_JSON_FEDERATION_ALLOW_ENV,
            configured,
        );
        Self {
            _lock: lock,
            previous,
        }
    }
}

impl Drop for MockHttpAllowGuard {
    fn drop(&mut self) {
        if let Some(previous) = self.previous.take() {
            std::env::set_var(crate::federation::HTTP_JSON_FEDERATION_ALLOW_ENV, previous);
        } else {
            std::env::remove_var(crate::federation::HTTP_JSON_FEDERATION_ALLOW_ENV);
        }
    }
}

