//! The graph version commit record (MEDULLA-FEDERATED-QUERY-R078.7).
//!
//! Every version bump records the wall-clock instant, in epoch milliseconds, at which
//! that version became visible. A read pinned to a snapshot revision then asks "when was
//! this revision committed" and answers a time-bounded question ("the latest point at or
//! before the snapshot") against that instant instead of the clock of the read.
//!
//! Retention follows the node-history bound (`graph.node_history_revisions`): a revision
//! older than the oldest retained record has no known commit instant.

use std::collections::VecDeque;
use std::sync::atomic::Ordering;

use parking_lot::Mutex;

use super::node_history::NODE_HISTORY_LIMIT;

/// The per-graph record of `(version, committed_at_ms)`, ascending by version.
#[derive(Debug, Default)]
pub struct VersionCommitLog {
    records: Mutex<VecDeque<(u64, i64)>>,
}

impl VersionCommitLog {
    /// Record that `version` became visible at `committed_at_ms`.
    pub fn record(&self, version: u64, committed_at_ms: i64) {
        let limit = NODE_HISTORY_LIMIT.load(Ordering::Acquire).max(1);
        let mut records = self.records.lock();
        // A version recorded twice (a durable jump after a bump) keeps its first instant.
        if records.back().is_some_and(|(last, _)| *last >= version) {
            return;
        }
        records.push_back((version, committed_at_ms));
        while records.len() > limit {
            records.pop_front();
        }
    }

    /// The commit instant of the newest recorded version at or before `version`, or
    /// `None` when no record reaches that far back.
    pub fn committed_at_ms(&self, version: u64) -> Option<i64> {
        let records = self.records.lock();
        let end = records.partition_point(|(recorded, _)| *recorded <= version);
        end.checked_sub(1).map(|index| records[index].1)
    }

    /// Forget every record: the image was replaced wholesale.
    pub fn reset(&self) {
        self.records.lock().clear();
    }
}

/// The current wall-clock instant in epoch milliseconds.
pub(super) fn now_epoch_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newest_record_at_or_before_the_version_answers() {
        let log = VersionCommitLog::default();
        log.record(3, 300);
        log.record(5, 500);
        log.record(5, 999);
        assert_eq!(log.committed_at_ms(2), None);
        assert_eq!(log.committed_at_ms(3), Some(300));
        assert_eq!(log.committed_at_ms(4), Some(300));
        assert_eq!(log.committed_at_ms(5), Some(500));
        assert_eq!(log.committed_at_ms(9), Some(500));
        log.reset();
        assert_eq!(log.committed_at_ms(9), None);
    }
}
