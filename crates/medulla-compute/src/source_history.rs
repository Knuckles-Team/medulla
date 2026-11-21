//! Derived history indexes of the source-changeset chain (MEDULLA-SCM-R005).
//!
//! Pure index code: the rows an accepted changeset adds to the branch
//! registry, the rebuild of those rows from a whole chain, and the reads over
//! them. The rows live in the branch registry owner table under
//! `hist:<registry_key>:<relative key>` and commit in the same transaction as
//! the changeset payload row (MEDULLA-SCM-R005.1), so no row lags or leads the
//! chain. The relative keys are built only by the key functions of this file:
//!
//! - `head` holds a [`HistoryHead`];
//! - `seq:<commit_id>` holds the commit's seq as 8 big-endian bytes;
//! - `id:<seq as 016x>` holds the commit id;
//! - `path:<byte length>:<path>:<seq as 016x>` holds an empty value.

use medulla_source_types::source_changeset::{
    Changeset, ChangesetRefusal, PathHistoryEntry, ReadPathHistoryReceipt, ReadPathHistoryRequest,
    SubmitRefusal,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The version of every row encoding of this file; a change to any row
/// encoding must bump it.
pub const SOURCE_HISTORY_INDEX_VERSION: u32 = 1;

/// The head row: how far the index has been written.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryHead {
    pub version: u32,
    /// The 1-based count of changesets on the chain up to and including
    /// `commit_id`.
    pub seq: u64,
    pub commit_id: String,
}

/// One index row: the relative key and its value.
pub type HistoryRow = (String, Vec<u8>);

/// Relative key of the head row.
pub fn head_key() -> String {
    "head".to_string()
}

/// Relative key of the seq row of one commit.
pub fn seq_key(commit_id: &str) -> String {
    format!("seq:{commit_id}")
}

/// Relative key of the commit-id row of one seq.
pub fn id_key(seq: u64) -> String {
    format!("id:{seq:016x}")
}

/// Relative key of the path-history row of `path` at `seq`.
pub fn path_key(path: &str, seq: u64) -> String {
    format!("{}{seq:016x}", path_prefix(path))
}

/// The prefix every path-history row of `path` shares.
pub fn path_prefix(path: &str) -> String {
    format!("path:{}:{path}:", path.len())
}

/// Encode the head row.
pub fn encode_head(head: &HistoryHead) -> Result<Vec<u8>, String> {
    rmp_serde::to_vec_named(head).map_err(|error| error.to_string())
}

/// Decode the head row.
pub fn decode_head(bytes: &[u8]) -> Result<HistoryHead, String> {
    rmp_serde::from_slice(bytes).map_err(|error| error.to_string())
}

fn decode_seq(bytes: &[u8]) -> Result<u64, String> {
    let array: [u8; 8] = bytes
        .try_into()
        .map_err(|_| "history seq row is not 8 bytes".to_string())?;
    Ok(u64::from_be_bytes(array))
}

/// The rows one changeset adds when it becomes the chain's next commit:
/// the new head, the seq and id rows and one `path:` row per changed path.
/// A rename arrives as a `Rename` entry at the new path plus a `Delete`
/// entry at the old path, so history does not follow renames.
pub fn apply_changeset(
    prev: Option<&HistoryHead>,
    commit_id: &str,
    changeset: &Changeset,
) -> Result<Vec<HistoryRow>, String> {
    let seq = prev.map_or(0, |head| head.seq) + 1;
    let head = HistoryHead {
        version: SOURCE_HISTORY_INDEX_VERSION,
        seq,
        commit_id: commit_id.to_string(),
    };
    let mut rows: Vec<HistoryRow> = vec![
        (head_key(), encode_head(&head)?),
        (seq_key(commit_id), seq.to_be_bytes().to_vec()),
        (id_key(seq), commit_id.as_bytes().to_vec()),
    ];
    for changed in &changeset.changed_paths {
        rows.push((path_key(&changed.path, seq), Vec::new()));
    }
    Ok(rows)
}

/// The rows of a whole chain, `chain` oldest first as `(commit id,
/// changeset)`. Equal to applying every changeset one at a time.
pub fn rebuild(chain: &[(String, Changeset)]) -> Result<Vec<HistoryRow>, String> {
    let mut rows: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    let mut head: Option<HistoryHead> = None;
    for (commit_id, changeset) in chain {
        for (key, value) in apply_changeset(head.as_ref(), commit_id, changeset)? {
            if key == head_key() {
                head = Some(decode_head(&value)?);
            }
            rows.insert(key, value);
        }
    }
    Ok(rows.into_iter().collect())
}

/// Read access to the stored index rows of one branch.
pub trait HistoryIndex {
    /// The value of one relative key.
    fn row(&self, key: &str) -> Result<Option<Vec<u8>>, String>;
    /// At most `limit` rows whose key starts with `prefix` and is at or below
    /// `upper`, newest key first.
    fn scan_rev(&self, prefix: &str, upper: &str, limit: usize)
        -> Result<Vec<(String, Vec<u8>)>, String>;
    /// The changeset the branch commit `commit_id` carries.
    fn changeset_at(&self, commit_id: &str) -> Result<Option<Changeset>, String>;
}

/// Why a history read produced no entries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HistoryError {
    /// A typed refusal.
    Refused(SubmitRefusal),
    /// The store failed.
    Store(String),
}

impl From<String> for HistoryError {
    fn from(error: String) -> Self {
        Self::Store(error)
    }
}

fn refused(refusal: ChangesetRefusal, detail: &str) -> HistoryError {
    HistoryError::Refused(SubmitRefusal::new(refusal, detail))
}

/// The head row, checked against the branch head: absent, of another version
/// or naming another commit refuses `HISTORY_INDEX_STALE`.
pub fn current_head(index: &dyn HistoryIndex, branch_head: &str) -> Result<HistoryHead, HistoryError> {
    let stale = || refused(ChangesetRefusal::HistoryIndexStale, "the history index is stale");
    let Some(bytes) = index.row(&head_key())? else {
        return Err(stale());
    };
    let head = decode_head(&bytes)?;
    if head.version != SOURCE_HISTORY_INDEX_VERSION || head.commit_id != branch_head {
        return Err(stale());
    }
    Ok(head)
}

/// The seq of the changeset commit `rev`, or `REVISION_NOT_ON_CHAIN`.
pub fn seq_of(index: &dyn HistoryIndex, rev: &str) -> Result<u64, HistoryError> {
    match index.row(&seq_key(rev))? {
        Some(bytes) => Ok(decode_seq(&bytes)?),
        None => Err(refused(
            ChangesetRefusal::RevisionNotOnChain,
            "rev is not a changeset of the branch",
        )),
    }
}

/// The changesets that touched `request.path`, newest first, with seq at or
/// below the seq of `request.rev`.
pub fn path_history(
    index: &dyn HistoryIndex,
    branch_head: &str,
    request: &ReadPathHistoryRequest,
) -> Result<ReadPathHistoryReceipt, HistoryError> {
    let limit = request.validate().map_err(HistoryError::Refused)?;
    current_head(index, branch_head)?;
    let seq = seq_of(index, &request.rev)?;
    let rows = index.scan_rev(&path_prefix(&request.path), &path_key(&request.path, seq), limit)?;
    let mut entries = Vec::with_capacity(rows.len());
    for (key, _) in rows {
        let hex = key
            .rsplit(':')
            .next()
            .ok_or_else(|| "malformed path history key".to_string())?;
        let row_seq = u64::from_str_radix(hex, 16).map_err(|error| error.to_string())?;
        let commit = index
            .row(&id_key(row_seq))?
            .ok_or_else(|| "history id row is missing".to_string())?;
        let commit = String::from_utf8(commit).map_err(|error| error.to_string())?;
        let changeset = index
            .changeset_at(&commit)?
            .ok_or_else(|| "history names a commit with no changeset".to_string())?;
        let op = changeset
            .changed_paths
            .iter()
            .find(|changed| changed.path == request.path)
            .map(|changed| changed.op)
            .ok_or_else(|| "history row names a path the changeset did not change".to_string())?;
        entries.push(PathHistoryEntry {
            commit,
            changeset_id: changeset.id,
            op,
            author_principal: changeset.author_principal,
            created_at: changeset.created_at,
            message: changeset.message,
        });
    }
    Ok(ReadPathHistoryReceipt { entries })
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_source_types::source_changeset::{ChangedPath, PathOp, Trailers};
    use std::collections::HashMap;

    struct Mem {
        rows: BTreeMap<String, Vec<u8>>,
        changesets: HashMap<String, Changeset>,
    }

    impl HistoryIndex for Mem {
        fn row(&self, key: &str) -> Result<Option<Vec<u8>>, String> {
            Ok(self.rows.get(key).cloned())
        }
        fn scan_rev(
            &self,
            prefix: &str,
            upper: &str,
            limit: usize,
        ) -> Result<Vec<(String, Vec<u8>)>, String> {
            Ok(self
                .rows
                .range(prefix.to_string()..=upper.to_string())
                .rev()
                .filter(|(key, _)| key.starts_with(prefix))
                .take(limit)
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect())
        }
        fn changeset_at(&self, commit_id: &str) -> Result<Option<Changeset>, String> {
            Ok(self.changesets.get(commit_id).cloned())
        }
    }

    fn changed(path: &str, op: PathOp) -> ChangedPath {
        ChangedPath {
            path: path.to_string(),
            op,
            old_blob: None,
            new_blob: (op != PathOp::Delete).then(|| "b".repeat(64)),
            mode: 0o100644,
        }
    }

    /// The 64-hex id of the commit carrying changeset `n`.
    fn commit(n: usize) -> String {
        format!("{:064x}", n + 1)
    }

    /// A chain of `count` changesets of adds, modifies, renames and deletes,
    /// oldest first, each carried by `commit(n)`.
    fn chain(count: usize) -> Vec<(String, Changeset)> {
        let mut out: Vec<(String, Changeset)> = Vec::new();
        for n in 0..count {
            let paths = match n % 5 {
                0 => vec![changed(&format!("d/f{n}"), PathOp::Add), changed("a", PathOp::Modify)],
                1 => vec![changed("a", PathOp::Modify)],
                2 => vec![changed("b", PathOp::Rename), changed("a", PathOp::Delete)],
                3 => vec![changed("a", PathOp::Add)],
                _ => vec![changed(&format!("d/f{}", n - 4), PathOp::Delete)],
            };
            let parent = out.last().map(|(id, _)| id.clone()).unwrap_or_default();
            let changeset = Changeset {
                id: String::new(),
                repo: "r".into(),
                parents: vec![parent],
                author_principal: "p".into(),
                message: format!("m{n}"),
                changed_paths: paths,
                created_at: n as u64,
                trailers: Trailers::default(),
                agent_identity: None,
                run_trace_id: None,
            }
            .sealed();
            out.push((commit(n), changeset));
        }
        out
    }

    fn mem_of(chain: &[(String, Changeset)]) -> Mem {
        Mem {
            rows: rebuild(chain).unwrap().into_iter().collect(),
            changesets: chain.iter().cloned().collect(),
        }
    }

    // spec: MEDULLA-SCM-R005
    #[test]
    fn incremental_rows_equal_rebuild() {
        let chain = chain(200);
        let mut rows: BTreeMap<String, Vec<u8>> = BTreeMap::new();
        let mut head: Option<HistoryHead> = None;
        for (commit_id, changeset) in &chain {
            for (key, value) in apply_changeset(head.as_ref(), commit_id, changeset).unwrap() {
                if key == head_key() {
                    head = Some(decode_head(&value).unwrap());
                }
                rows.insert(key, value);
            }
        }
        let rebuilt: BTreeMap<String, Vec<u8>> = rebuild(&chain).unwrap().into_iter().collect();
        assert_eq!(rows, rebuilt);
        assert_eq!(head.unwrap().seq, 200);
    }

    // spec: MEDULLA-SCM-R005.1
    #[test]
    fn path_history_lists_newest_first_up_to_rev() {
        let chain = chain(10);
        let mem = mem_of(&chain);
        let head = commit(9);
        let request = |n: usize, limit: u32| ReadPathHistoryRequest {
            repo: "r".into(),
            branch: "b".into(),
            rev: commit(n),
            path: "a".into(),
            limit,
        };
        let all = path_history(&mem, &head, &request(6, 0)).unwrap();
        let messages: Vec<&str> = all.entries.iter().map(|e| e.message.as_str()).collect();
        assert_eq!(messages, ["m6", "m5", "m3", "m2", "m1", "m0"]);
        let ops: Vec<PathOp> = all.entries.iter().take(4).map(|e| e.op).collect();
        assert_eq!(
            ops,
            [PathOp::Modify, PathOp::Modify, PathOp::Add, PathOp::Delete]
        );
        assert_eq!(all.entries[0].commit, commit(6));
        let capped = path_history(&mem, &head, &request(6, 3)).unwrap();
        assert_eq!(capped.entries.len(), 3);
        assert_eq!(capped.entries[2].message, "m3");
        // A path no changeset touched has no history.
        let mut untouched = request(6, 0);
        untouched.path = "zzz".into();
        assert!(path_history(&mem, &head, &untouched).unwrap().entries.is_empty());
        // A limit over the maximum, an unknown revision and a stale head refuse.
        assert!(matches!(
            path_history(&mem, &head, &request(6, 1001)),
            Err(HistoryError::Refused(r)) if r.refusal == ChangesetRefusal::ChangesetInvalid
        ));
        let mut unknown = request(6, 0);
        unknown.rev = "f".repeat(64);
        assert!(matches!(
            path_history(&mem, &head, &unknown),
            Err(HistoryError::Refused(r)) if r.refusal == ChangesetRefusal::RevisionNotOnChain
        ));
        assert!(matches!(
            path_history(&mem, &commit(8), &request(6, 0)),
            Err(HistoryError::Refused(r)) if r.refusal == ChangesetRefusal::HistoryIndexStale
        ));
    }
}
