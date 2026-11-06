//! Spill of incremental view state (MEDULLA-INCREMENTAL-VIEWS-R007.3).
//!
//! The manager keeps resident circuits under `views.memory_bytes`. A view that falls out
//! of the budget is encoded, held as `pending` until the store acknowledges the write, and
//! reloaded on its next change or read.

use std::sync::Arc;

/// The store behind spilled view state.
pub trait ViewStateSpill: Send + Sync {
    /// Persist `bytes` as the operator state of view `name`. The sink calls
    /// `PlanMatViewManager::spill_acknowledged` once the write is durable.
    fn put(&self, name: &str, bytes: Arc<Vec<u8>>);
    /// The persisted operator state of view `name`.
    fn get(&self, name: &str) -> Option<Vec<u8>>;
}

/// Share of the memory budget a spill pass stops below.
const SETTLE_PERCENT: u64 = 90;

/// How many bytes a spill pass has to get under.
pub(super) fn settle_target(budget: u64) -> u64 {
    budget / 100 * SETTLE_PERCENT
}
