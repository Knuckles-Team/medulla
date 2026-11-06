//! The view status board (MEDULLA-INCREMENTAL-VIEWS-R005.1).
//!
//! A process-global, name-keyed board of materialized-view states. The view
//! manager publishes each state change; the `medulla_catalog.matviews` system
//! table reads a snapshot. Plain data plus three functions.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

/// Which kind of definition a view came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ViewFamily {
    /// A plan-backed view.
    Plan,
    /// A SQL view.
    Sql,
}

impl ViewFamily {
    /// The closed label text of the family.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Plan => "plan",
            Self::Sql => "sql",
        }
    }
}

/// How a view is maintained now.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ViewMode {
    /// Folded change by change.
    Incremental,
    /// Recomputed on read.
    Recompute,
    /// Maintained state spilled out of memory.
    Spilled,
}

impl ViewMode {
    /// The closed label text of the mode.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Incremental => "incremental",
            Self::Recompute => "recompute",
            Self::Spilled => "spilled",
        }
    }
}

/// One view's published state.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ViewStatus {
    /// The view name; the board key.
    pub name: String,
    /// Where the definition came from.
    pub family: ViewFamily,
    /// The graph the view reads.
    pub graph: String,
    /// The current maintenance mode.
    pub mode: ViewMode,
    /// The registered refusal code behind a fallback, if any.
    pub reason_code: Option<String>,
    /// The human text behind a fallback, if any.
    pub reason: Option<String>,
    /// Maintained state in bytes.
    pub state_bytes: u64,
    /// Changes folded so far.
    pub applied_changes: u64,
    /// Shadow recompute checks run.
    pub shadow_checks: u64,
    /// Shadow checks that disagreed.
    pub shadow_mismatches: u64,
}

static BOARD: OnceLock<RwLock<BTreeMap<String, ViewStatus>>> = OnceLock::new();

fn board() -> &'static RwLock<BTreeMap<String, ViewStatus>> {
    BOARD.get_or_init(|| RwLock::new(BTreeMap::new()))
}

/// Publish a view's state, replacing any earlier entry of the same name.
pub fn publish(status: ViewStatus) {
    board().write().insert(status.name.clone(), status);
}

/// Remove a view's entry; an unknown name is a no-op.
pub fn remove(name: &str) {
    board().write().remove(name);
}

/// Every published view, in name order.
pub fn snapshot() -> Vec<ViewStatus> {
    board().read().values().cloned().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(name: &str, mode: ViewMode, applied_changes: u64) -> ViewStatus {
        ViewStatus {
            name: name.to_string(),
            family: ViewFamily::Plan,
            graph: "g".to_string(),
            mode,
            reason_code: None,
            reason: None,
            state_bytes: 0,
            applied_changes,
            shadow_checks: 0,
            shadow_mismatches: 0,
        }
    }

    fn find(name: &str) -> Option<ViewStatus> {
        snapshot().into_iter().find(|s| s.name == name)
    }

    // spec: MEDULLA-INCREMENTAL-VIEWS-R005.1
    /// Verifies MEDULLA-INCREMENTAL-VIEWS-R005.1: Model: the view status board
    #[test]
    fn publish_replaces_and_remove_deletes() {
        let name = "view_status_test_view";
        publish(status(name, ViewMode::Incremental, 1));
        publish(status(name, ViewMode::Recompute, 7));
        let all: Vec<_> = snapshot().into_iter().filter(|s| s.name == name).collect();
        assert_eq!(all, vec![status(name, ViewMode::Recompute, 7)]);
        remove(name);
        assert_eq!(find(name), None);
        remove(name);
    }
}
