//! Optimistic concurrency footprints (MEDULLA-ENGINE-PARITY-R002.1, R004.1).
//!
//! A transaction reads from the snapshot at `snapshot_seq` and records the
//! table key ranges it read and wrote. At commit the footprint validates
//! against the writes committed after its snapshot: an overlap with either
//! set is a conflict. Footprints that do not overlap are `disjoint` and
//! validate without waiting on each other.

/// A half-open key range `[start, end)` within one table. An empty `end` is
/// unbounded above, so the range `start = [], end = []` covers the whole table.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct KeyRange {
    pub table: String,
    pub start: Vec<u8>,
    pub end: Vec<u8>,
}

impl KeyRange {
    /// The range holding exactly `key`.
    pub fn point(table: &str, key: &[u8]) -> Self {
        let mut end = key.to_vec();
        end.push(0);
        Self {
            table: table.to_string(),
            start: key.to_vec(),
            end,
        }
    }

    /// The range covering every key of `table` (DDL, truncate).
    pub fn whole_table(table: &str) -> Self {
        Self {
            table: table.to_string(),
            start: Vec::new(),
            end: Vec::new(),
        }
    }

    /// Whether the two ranges share at least one key.
    pub fn overlaps(&self, other: &KeyRange) -> bool {
        let below = |start: &[u8], end: &[u8]| end.is_empty() || start < end;
        self.table == other.table
            && below(&self.start, &other.end)
            && below(&other.start, &self.end)
    }
}

/// The read and write sets of one transaction over its snapshot.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TxnFootprint {
    pub snapshot_seq: u64,
    pub reads: Vec<KeyRange>,
    pub writes: Vec<KeyRange>,
}

/// One committed transaction's write set.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommittedWrites {
    pub commit_seq: u64,
    pub writes: Vec<KeyRange>,
}

/// `TXN_CONFLICT`: the keys of the footprint that changed after its snapshot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TxnConflict {
    pub keys: Vec<KeyRange>,
}

impl TxnConflict {
    pub const CODE: &'static str = "TXN_CONFLICT";
}

impl std::fmt::Display for TxnConflict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}: {} key range(s) changed",
            Self::CODE,
            self.keys.len()
        )
    }
}

impl std::error::Error for TxnConflict {}

fn touched(footprint: &TxnFootprint) -> impl Iterator<Item = &KeyRange> {
    footprint.reads.iter().chain(footprint.writes.iter())
}

/// Whether two footprints share no key: neither writes a range the other
/// reads or writes.
pub fn disjoint(a: &TxnFootprint, b: &TxnFootprint) -> bool {
    let clashes = |writer: &TxnFootprint, other: &TxnFootprint| {
        writer
            .writes
            .iter()
            .any(|w| touched(other).any(|k| k.overlaps(w)))
    };
    !clashes(a, b) && !clashes(b, a)
}

impl TxnFootprint {
    /// Validate against `committed`: refuse with every footprint key that a
    /// commit after `snapshot_seq` wrote.
    pub fn validate(&self, committed: &[CommittedWrites]) -> Result<(), TxnConflict> {
        let mut keys: Vec<KeyRange> = Vec::new();
        for commit in committed
            .iter()
            .filter(|c| c.commit_seq > self.snapshot_seq)
        {
            for key in touched(self) {
                if commit.writes.iter().any(|w| w.overlaps(key)) && !keys.contains(key) {
                    keys.push(key.clone());
                }
            }
        }
        if keys.is_empty() {
            Ok(())
        } else {
            keys.sort();
            Err(TxnConflict { keys })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{CommittedWrites, KeyRange, TxnConflict, TxnFootprint};

    fn key(table: &str, k: &str) -> KeyRange {
        KeyRange::point(table, k.as_bytes())
    }

    fn range(table: &str, start: &[u8], end: &[u8]) -> KeyRange {
        KeyRange {
            table: table.to_string(),
            start: start.to_vec(),
            end: end.to_vec(),
        }
    }

    // spec: MEDULLA-ENGINE-PARITY-R002.1
    #[test]
    fn point_range_holds_exactly_its_key() {
        let p = key("oncall", "x");
        assert_eq!(p.start, b"x".to_vec());
        assert_eq!(p.end, b"x\0".to_vec());
        assert!(p.overlaps(&p));
        // A wider range over the same table covers the point.
        assert!(p.overlaps(&range("oncall", b"a", b"z")));
        // A sibling point, a later half-open range, and a foreign table
        // do not share a key.
        assert!(!p.overlaps(&key("oncall", "y")));
        assert!(!p.overlaps(&range("oncall", b"y", b"z")));
        assert!(!p.overlaps(&key("other", "x")));
        // Half-open: a range ending where the point begins is disjoint.
        assert!(!range("oncall", b"a", b"x").overlaps(&p));
    }

    // spec: MEDULLA-ENGINE-PARITY-R002.2
    #[test]
    fn whole_table_range_covers_every_key_of_its_table_only() {
        let all = KeyRange::whole_table("acct");
        assert!(all.overlaps(&key("acct", "anything")));
        assert!(key("acct", "anything").overlaps(&all));
        assert!(all.overlaps(&range("acct", b"a", b"b")));
        assert!(all.overlaps(&KeyRange::whole_table("acct")));
        assert!(!all.overlaps(&key("other", "anything")));
    }

    // spec: MEDULLA-ENGINE-PARITY-R002.1
    #[test]
    fn validate_refuses_only_commits_after_the_snapshot() {
        let fp = TxnFootprint {
            snapshot_seq: 5,
            reads: vec![key("acct", "r1")],
            writes: vec![key("acct", "w1")],
        };
        // A commit at the snapshot seq is already visible to the snapshot.
        let at = vec![CommittedWrites {
            commit_seq: 5,
            writes: vec![key("acct", "w1"), key("acct", "r1")],
        }];
        assert_eq!(fp.validate(&at), Ok(()));
        // A later commit that touched a read key or a written key refuses.
        let after_read = vec![CommittedWrites {
            commit_seq: 6,
            writes: vec![key("acct", "r1")],
        }];
        assert_eq!(
            fp.validate(&after_read).unwrap_err().keys,
            vec![key("acct", "r1")]
        );
        let after_write = vec![CommittedWrites {
            commit_seq: 6,
            writes: vec![key("acct", "w1")],
        }];
        assert_eq!(
            fp.validate(&after_write).unwrap_err().keys,
            vec![key("acct", "w1")]
        );
        // A later commit that touched only an untouched key passes.
        let after_other = vec![CommittedWrites {
            commit_seq: 7,
            writes: vec![key("acct", "r2")],
        }];
        assert_eq!(fp.validate(&after_other), Ok(()));
        // An empty history is a fresh snapshot: nothing to check.
        assert_eq!(fp.validate(&[]), Ok(()));
    }

    // spec: MEDULLA-ENGINE-PARITY-R002.1
    #[test]
    fn conflict_deduplicates_and_sorts_the_changed_keys() {
        let fp = TxnFootprint {
            snapshot_seq: 1,
            reads: vec![key("acct", "b")],
            writes: vec![key("acct", "a")],
        };
        // Two later commits touch both keys again: each key is listed once,
        // sorted, with the refusal code.
        let log = vec![
            CommittedWrites {
                commit_seq: 2,
                writes: vec![key("acct", "a"), key("acct", "b")],
            },
            CommittedWrites {
                commit_seq: 3,
                writes: vec![key("acct", "a"), key("acct", "b")],
            },
        ];
        let err = fp.validate(&log).unwrap_err();
        assert_eq!(err.keys, vec![key("acct", "a"), key("acct", "b")]);
        assert_eq!(TxnConflict::CODE, "TXN_CONFLICT");
        assert!(err.to_string().starts_with("TXN_CONFLICT"));
    }
}
