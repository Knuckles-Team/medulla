//! Commit footprints of user-table transactions (MEDULLA-ENGINE-PARITY-R002.2).
//!
//! `TableStore` keeps one [`OccLog`]: a `commit_seq` counter and a ring of the
//! write sets of the last `tables.occ_history_commits` commits. A transaction
//! pinned at `BEGIN` carries the `commit_seq` it saw. At commit, inside the
//! write transaction, its footprint validates against the ring: a key it read
//! or wrote that a later commit wrote refuses the commit and nothing is kept.

use std::collections::VecDeque;
use std::sync::Mutex;

use medulla_tables::schema::{Cell, TableConstraint, TableSchema};
use medulla_transaction::occ::{CommittedWrites, KeyRange, TxnFootprint};

/// Refusal when a conflicting later commit wrote a footprint key.
pub const TXN_CONFLICT: &str = "TXN_CONFLICT";
/// Refusal when the snapshot is older than the retained history.
pub const TXN_SNAPSHOT_EXPIRED: &str = "TXN_SNAPSHOT_EXPIRED";

/// Default, lower and upper bound of `tables.occ_history_commits` (commits).
pub const OCC_HISTORY_DEFAULT: usize = 4096;
pub const OCC_HISTORY_MIN: usize = 64;
pub const OCC_HISTORY_MAX: usize = 1_048_576;
/// Environment spelling of the config key `tables.occ_history_commits`.
pub const OCC_HISTORY_ENV: &str = "MEDULLA_TABLES_OCC_HISTORY_COMMITS";

/// Resolve the ring size from a raw value: absent or unparsable is the
/// default; any parsed value clamps into the bounds.
pub fn parse_history_commits(raw: Option<&str>) -> usize {
    raw.and_then(|v| v.trim().parse::<usize>().ok())
        .map_or(OCC_HISTORY_DEFAULT, |n| {
            n.clamp(OCC_HISTORY_MIN, OCC_HISTORY_MAX)
        })
}

#[derive(Debug, Default)]
struct OccState {
    commit_seq: u64,
    /// The highest commit sequence that fell off the ring.
    evicted_through: u64,
    ring: VecDeque<CommittedWrites>,
}

/// The commit sequence counter and the ring of recent write sets.
#[derive(Debug)]
pub struct OccLog {
    history: usize,
    state: Mutex<OccState>,
}

impl Default for OccLog {
    fn default() -> Self {
        Self::with_history(parse_history_commits(
            std::env::var(OCC_HISTORY_ENV).ok().as_deref(),
        ))
    }
}

impl OccLog {
    pub fn with_history(history: usize) -> Self {
        Self {
            history: history.clamp(OCC_HISTORY_MIN, OCC_HISTORY_MAX),
            state: Mutex::new(OccState::default()),
        }
    }

    fn state(&self) -> std::sync::MutexGuard<'_, OccState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The sequence of the latest recorded commit: the snapshot a `BEGIN` pins.
    pub fn commit_seq(&self) -> u64 {
        self.state().commit_seq
    }

    /// Validate `footprint`; a refusal is the prefixed code and message.
    pub fn validate(&self, footprint: &TxnFootprint) -> Result<(), String> {
        let state = self.state();
        if footprint.snapshot_seq < state.evicted_through {
            return Err(format!(
                "{TXN_SNAPSHOT_EXPIRED}: snapshot {} is older than the {} retained commits",
                footprint.snapshot_seq, self.history
            ));
        }
        let ring: Vec<CommittedWrites> = state.ring.iter().cloned().collect();
        footprint.validate(&ring).map_err(|conflict| {
            let keys: Vec<String> = conflict
                .keys
                .iter()
                .map(|k| format!("{}/{}", k.table, hex(&k.start)))
                .collect();
            format!(
                "{TXN_CONFLICT}: keys changed after snapshot: {}",
                keys.join(", ")
            )
        })
    }

    /// Record the write set of a commit in progress; returns its sequence.
    /// Called inside the serialized write transaction, so no other commit
    /// validates between this and the commit point.
    pub fn record(&self, writes: Vec<KeyRange>) -> Option<u64> {
        if writes.is_empty() {
            return None;
        }
        let mut state = self.state();
        state.commit_seq += 1;
        let commit_seq = state.commit_seq;
        state.ring.push_back(CommittedWrites { commit_seq, writes });
        while state.ring.len() > self.history {
            if let Some(old) = state.ring.pop_front() {
                state.evicted_through = old.commit_seq;
            }
        }
        Some(commit_seq)
    }

    /// Undo [`Self::record`] for a commit that did not reach its commit point.
    pub fn retract(&self, commit_seq: u64) {
        let mut state = self.state();
        if state.commit_seq == commit_seq {
            if let Some(last) = state.ring.back() {
                if last.commit_seq == commit_seq {
                    state.ring.pop_back();
                    state.commit_seq -= 1;
                    return;
                }
            }
        }
        // Not the latest (cannot happen under the serialized write): keep it.
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The primary-key column indices of `schema` (column flags and the
/// table-level constraint), empty for a table without one.
fn key_columns(schema: &TableSchema) -> Vec<usize> {
    let mut cols: Vec<usize> = schema
        .columns()
        .iter()
        .enumerate()
        .filter(|(_, c)| c.primary_key)
        .map(|(i, _)| i)
        .collect();
    for c in schema.constraints() {
        if let TableConstraint::PrimaryKey { columns, .. } = c {
            cols.extend(columns.iter().filter_map(|n| schema.column_index(n)));
        }
    }
    cols.sort_unstable();
    cols.dedup();
    cols
}

/// The key identity of one row: its primary-key cells, or, for a table
/// without a primary key, the whole row.
fn row_key(cols: &[usize], cells: &[Cell]) -> Vec<u8> {
    let picked: Vec<&Cell> = if cols.is_empty() {
        cells.iter().collect()
    } else {
        cols.iter().filter_map(|i| cells.get(*i)).collect()
    };
    rmp_serde::to_vec_named(&picked).unwrap_or_default()
}

/// The keys a transaction read and wrote while applying its ops.
#[derive(Debug, Default)]
pub(super) struct FootprintAccum {
    pub reads: Vec<KeyRange>,
    pub writes: Vec<KeyRange>,
}

impl FootprintAccum {
    /// An insert writes the new row's key. A table without a primary key has no
    /// key identity for a new row, so concurrent inserts never conflict.
    pub fn inserted(&mut self, table: &str, schema: &TableSchema, rows: &[Vec<Cell>]) {
        let cols = key_columns(schema);
        if cols.is_empty() {
            return;
        }
        for cells in rows {
            self.writes
                .push(KeyRange::point(table, &row_key(&cols, cells)));
        }
    }

    /// An update reads the key of each matched row and writes its old and new key.
    pub fn updated(
        &mut self,
        table: &str,
        schema: &TableSchema,
        changes: &[(Vec<Cell>, Vec<Cell>)],
    ) {
        let cols = key_columns(schema);
        for (old, new) in changes {
            let old_key = KeyRange::point(table, &row_key(&cols, old));
            self.reads.push(old_key.clone());
            self.writes.push(old_key);
            self.writes
                .push(KeyRange::point(table, &row_key(&cols, new)));
        }
    }

    /// A delete reads and writes the key of each removed row.
    pub fn deleted(&mut self, table: &str, schema: &TableSchema, removed: &[Vec<Cell>]) {
        let cols = key_columns(schema);
        for cells in removed {
            let key = KeyRange::point(table, &row_key(&cols, cells));
            self.reads.push(key.clone());
            self.writes.push(key);
        }
    }

    /// DDL rewrites a table: the full-table range.
    pub fn ddl(&mut self, table: &str) {
        self.writes.push(KeyRange::whole_table(table));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn point(k: &str) -> KeyRange {
        KeyRange::point("t", k.as_bytes())
    }

    // spec: MEDULLA-ENGINE-PARITY-R002.2
    #[test]
    fn history_config_defaults_and_clamps() {
        assert_eq!(parse_history_commits(None), 4096);
        assert_eq!(parse_history_commits(Some("junk")), 4096);
        assert_eq!(parse_history_commits(Some("1")), 64);
        assert_eq!(parse_history_commits(Some("999999999")), 1_048_576);
        assert_eq!(parse_history_commits(Some(" 500 ")), 500);
    }

    // spec: MEDULLA-ENGINE-PARITY-R002.2
    #[test]
    fn conflict_names_keys_and_expiry_refuses_old_snapshots() {
        let log = OccLog::with_history(64);
        let pinned = log.commit_seq();
        assert_eq!(log.record(vec![point("a")]), Some(1));
        let fp = TxnFootprint {
            snapshot_seq: pinned,
            reads: vec![point("a")],
            writes: vec![point("a")],
        };
        let err = log.validate(&fp).unwrap_err();
        assert!(err.starts_with("TXN_CONFLICT"), "{err}");
        assert!(err.contains("t/61"), "{err}");
        let disjoint = TxnFootprint {
            snapshot_seq: pinned,
            reads: vec![point("b")],
            writes: vec![point("b")],
        };
        assert_eq!(log.validate(&disjoint), Ok(()));
        // Push the pinned snapshot out of the ring.
        for i in 0..70 {
            log.record(vec![point(&format!("k{i}"))]);
        }
        let err = log.validate(&disjoint).unwrap_err();
        assert!(err.starts_with("TXN_SNAPSHOT_EXPIRED"), "{err}");
    }

    // spec: MEDULLA-ENGINE-PARITY-R002.2
    #[test]
    fn retract_undoes_only_the_commit_in_progress() {
        let log = OccLog::with_history(64);
        let seq = log.record(vec![point("a")]).unwrap();
        log.retract(seq);
        assert_eq!(log.commit_seq(), 0);
        assert_eq!(log.record(vec![]), None);
    }
}
