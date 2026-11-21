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
//! - `path:<byte length>:<path>:<seq as 016x>` holds an empty value;
//! - `blame:<byte length>:<path>:<seq as 016x>` holds a [`BlameEntry`]
//!   (MEDULLA-SCM-R005.2).

use crate::line_merge::{diff_chunks, is_binary, split};
use medulla_source_types::source_changeset::{
    BlameRun, Changeset, ChangesetRefusal, PathHistoryEntry, PathOp, ReadBlameReceipt,
    ReadBlameRequest, ReadPathHistoryReceipt, ReadPathHistoryRequest, SubmitRefusal,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The version of every row encoding of this file; a change to any row
/// encoding must bump it.
pub const SOURCE_HISTORY_INDEX_VERSION: u32 = 1;

/// The most lines of one side of a blamed file; a larger file writes state
/// [`BlameState::TooLarge`], which bounds the diff table (`Fixed`).
pub const BLAME_MAX_LINES: usize = 2048;

/// How a blame row classifies a path version; it has no transitions.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlameState {
    /// A text file whose line origins are in `runs`.
    Indexed,
    /// The new blob is binary.
    Binary,
    /// A side of the diff has more than [`BLAME_MAX_LINES`] lines.
    TooLarge,
    /// The path was deleted at this seq.
    Deleted,
}

/// The value of a `blame:` row. Each run is `(line_count, origin_seq)` and the
/// runs cover the file's lines in order.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlameEntry {
    pub state: BlameState,
    pub runs: Vec<(u32, u64)>,
}

/// What `apply_changeset` reads beyond the changeset itself.
pub struct BlameInputs<'a> {
    /// The bytes stored under a blob digest.
    pub blob: &'a dyn Fn(&str) -> Result<Vec<u8>, String>,
    /// The newest blame row of a path below the changeset being applied.
    pub prior: &'a dyn Fn(&str) -> Result<Option<BlameEntry>, String>,
}

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

/// Relative key of the blame row of `path` at `seq`.
pub fn blame_key(path: &str, seq: u64) -> String {
    format!("{}{seq:016x}", blame_prefix(path))
}

/// The prefix every blame row of `path` shares.
pub fn blame_prefix(path: &str) -> String {
    format!("blame:{}:{path}:", path.len())
}

/// Encode a blame row.
pub fn encode_blame(entry: &BlameEntry) -> Result<Vec<u8>, String> {
    rmp_serde::to_vec_named(entry).map_err(|error| error.to_string())
}

/// Decode a blame row.
pub fn decode_blame(bytes: &[u8]) -> Result<BlameEntry, String> {
    rmp_serde::from_slice(bytes).map_err(|error| error.to_string())
}

/// The paths whose newest earlier blame row `apply_changeset` reads: every
/// `Modify` path, and the old path of a `Rename` (the `Delete` entry that
/// immediately follows it).
pub fn prior_blame_paths(changeset: &Changeset) -> Vec<String> {
    let mut paths = Vec::new();
    for (index, changed) in changeset.changed_paths.iter().enumerate() {
        match changed.op {
            PathOp::Modify => paths.push(changed.path.clone()),
            PathOp::Rename => {
                if let Some(next) = changeset.changed_paths.get(index + 1) {
                    paths.push(next.path.clone());
                }
            }
            PathOp::Add | PathOp::Delete => {}
        }
    }
    paths
}

/// The seq stored in a `seq:` row.
pub fn decode_seq(bytes: &[u8]) -> Result<u64, String> {
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
    inputs: &BlameInputs<'_>,
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
    for (index, changed) in changeset.changed_paths.iter().enumerate() {
        let entry = blame_entry(seq, changeset, index, inputs)?;
        rows.push((blame_key(&changed.path, seq), encode_blame(&entry)?));
    }
    Ok(rows)
}

fn single_run(lines: &[String], seq: u64) -> BlameEntry {
    BlameEntry {
        state: BlameState::Indexed,
        runs: if lines.is_empty() {
            Vec::new()
        } else {
            vec![(lines.len() as u32, seq)]
        },
    }
}

fn state_only(state: BlameState) -> BlameEntry {
    BlameEntry {
        state,
        runs: Vec::new(),
    }
}

/// The blame row of changed path number `index` of `changeset` at `seq`.
///
/// `Add` gives every line origin `seq`. `Modify` maps the newest earlier blame
/// row of the path through the line diff of `old_blob` to `new_blob`; an
/// unchanged line keeps its origin and an inserted or replaced line takes
/// `seq`. `Rename` takes the earlier row of the `Delete` entry that follows
/// it, then applies the `Modify` rule. `Delete` is `Deleted`. A binary new
/// blob is `Binary`; more than [`BLAME_MAX_LINES`] lines on either side is
/// `TooLarge`. When no indexed earlier row covers the old blob, every line
/// takes `seq`.
fn blame_entry(
    seq: u64,
    changeset: &Changeset,
    index: usize,
    inputs: &BlameInputs<'_>,
) -> Result<BlameEntry, String> {
    let changed = &changeset.changed_paths[index];
    let new_digest = match (&changed.op, &changed.new_blob) {
        (PathOp::Delete, _) | (_, None) => return Ok(state_only(BlameState::Deleted)),
        (_, Some(digest)) => digest,
    };
    let new_bytes = (inputs.blob)(new_digest)?;
    if is_binary(&new_bytes) {
        return Ok(state_only(BlameState::Binary));
    }
    let (new_lines, _) = split(&new_bytes);
    if new_lines.len() > BLAME_MAX_LINES {
        return Ok(state_only(BlameState::TooLarge));
    }
    let parent_path = match changed.op {
        PathOp::Modify => Some(changed.path.as_str()),
        PathOp::Rename => changeset
            .changed_paths
            .get(index + 1)
            .filter(|next| next.op == PathOp::Delete)
            .map(|next| next.path.as_str()),
        PathOp::Add | PathOp::Delete => None,
    };
    let Some(parent_path) = parent_path else {
        return Ok(single_run(&new_lines, seq));
    };
    let (Some(prior), Some(old_digest)) = ((inputs.prior)(parent_path)?, &changed.old_blob) else {
        return Ok(single_run(&new_lines, seq));
    };
    if prior.state != BlameState::Indexed {
        return Ok(single_run(&new_lines, seq));
    }
    let old_bytes = (inputs.blob)(old_digest)?;
    let (old_lines, _) = split(&old_bytes);
    if old_lines.len() > BLAME_MAX_LINES {
        return Ok(state_only(BlameState::TooLarge));
    }
    let covered: u64 = prior.runs.iter().map(|(count, _)| u64::from(*count)).sum();
    if is_binary(&old_bytes) || covered != old_lines.len() as u64 {
        return Ok(single_run(&new_lines, seq));
    }
    let mut origins: Vec<u64> = Vec::with_capacity(old_lines.len());
    for (count, origin) in &prior.runs {
        origins.extend(std::iter::repeat(*origin).take(*count as usize));
    }
    let mut mapped: Vec<u64> = Vec::with_capacity(new_lines.len());
    let mut cursor = 0usize;
    for (start, length, replacement) in diff_chunks(&old_lines, &new_lines) {
        mapped.extend_from_slice(&origins[cursor..start]);
        mapped.extend(std::iter::repeat(seq).take(replacement.len()));
        cursor = start + length;
    }
    mapped.extend_from_slice(&origins[cursor..]);
    let mut runs: Vec<(u32, u64)> = Vec::new();
    for origin in mapped {
        match runs.last_mut() {
            Some((count, last)) if *last == origin => *count += 1,
            _ => runs.push((1, origin)),
        }
    }
    Ok(BlameEntry {
        state: BlameState::Indexed,
        runs,
    })
}

/// The rows of a whole chain, `chain` oldest first as `(commit id,
/// changeset)`. Equal to applying every changeset one at a time.
pub fn rebuild(
    chain: &[(String, Changeset)],
    blob: &dyn Fn(&str) -> Result<Vec<u8>, String>,
) -> Result<Vec<HistoryRow>, String> {
    let mut rows: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    let mut touched: std::collections::HashMap<String, u64> = std::collections::HashMap::new();
    let mut head: Option<HistoryHead> = None;
    for (commit_id, changeset) in chain {
        let applied = {
            let prior = |path: &str| -> Result<Option<BlameEntry>, String> {
                let Some(seq) = touched.get(path) else {
                    return Ok(None);
                };
                rows.get(&blame_key(path, *seq))
                    .map(|bytes| decode_blame(bytes))
                    .transpose()
            };
            apply_changeset(
                head.as_ref(),
                commit_id,
                changeset,
                &BlameInputs {
                    blob,
                    prior: &prior,
                },
            )?
        };
        for (key, value) in applied {
            if key == head_key() {
                head = Some(decode_head(&value)?);
            }
            rows.insert(key, value);
        }
        let seq = head.as_ref().map_or(0, |head| head.seq);
        for changed in &changeset.changed_paths {
            touched.insert(changed.path.clone(), seq);
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

/// The origin of every line of `request.path` at `request.rev`.
///
/// Reads the newest blame row of the path at or below the seq of `rev`.
/// Refusals: `BLAME_BINARY`, `BLAME_FILE_TOO_LARGE`, `BLAME_PATH_ABSENT` (state
/// `Deleted` or no row), plus those of [`path_history`].
pub fn blame(
    index: &dyn HistoryIndex,
    branch_head: &str,
    request: &ReadBlameRequest,
) -> Result<ReadBlameReceipt, HistoryError> {
    request.validate().map_err(HistoryError::Refused)?;
    current_head(index, branch_head)?;
    let seq = seq_of(index, &request.rev)?;
    let rows = index.scan_rev(&blame_prefix(&request.path), &blame_key(&request.path, seq), 1)?;
    let absent = || refused(ChangesetRefusal::BlamePathAbsent, "the path has no blame at the revision");
    let Some((_, bytes)) = rows.into_iter().next() else {
        return Err(absent());
    };
    let entry = decode_blame(&bytes)?;
    match entry.state {
        BlameState::Binary => Err(refused(ChangesetRefusal::BlameBinary, "the file is binary")),
        BlameState::TooLarge => Err(refused(
            ChangesetRefusal::BlameFileTooLarge,
            "the file has too many lines to blame",
        )),
        BlameState::Deleted => Err(absent()),
        BlameState::Indexed => {
            let mut runs = Vec::with_capacity(entry.runs.len());
            let mut start_line = 0u32;
            for (line_count, origin) in entry.runs {
                let commit = index
                    .row(&id_key(origin))?
                    .ok_or_else(|| "history id row is missing".to_string())?;
                let commit = String::from_utf8(commit).map_err(|error| error.to_string())?;
                let changeset = index
                    .changeset_at(&commit)?
                    .ok_or_else(|| "history names a commit with no changeset".to_string())?;
                runs.push(BlameRun {
                    start_line,
                    line_count,
                    commit,
                    changeset_id: changeset.id,
                });
                start_line += line_count;
            }
            Ok(ReadBlameReceipt { runs })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_source_types::source_changeset::{ChangedPath, Trailers};
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

    type Blobs = HashMap<String, Vec<u8>>;

    /// The 64-hex id of the commit carrying changeset `n`.
    fn commit(n: usize) -> String {
        format!("{:064x}", n + 1)
    }

    fn blob_reader(blobs: &Blobs) -> impl Fn(&str) -> Result<Vec<u8>, String> + '_ {
        |digest: &str| {
            blobs
                .get(digest)
                .cloned()
                .ok_or_else(|| format!("no blob {digest}"))
        }
    }

    fn changed(path: &str, op: PathOp, old: Option<&str>, new: Option<&str>) -> ChangedPath {
        ChangedPath {
            path: path.to_string(),
            op,
            old_blob: old.map(str::to_string),
            new_blob: new.map(str::to_string),
            mode: 0o100644,
        }
    }

    fn changeset(n: usize, parent: &str, paths: Vec<ChangedPath>) -> Changeset {
        Changeset {
            id: String::new(),
            repo: "r".into(),
            parents: vec![parent.to_string()],
            author_principal: "p".into(),
            message: format!("m{n}"),
            changed_paths: paths,
            created_at: n as u64,
            trailers: Trailers::default(),
            agent_identity: None,
            run_trace_id: None,
        }
        .sealed()
    }

    /// Six lines, the `n % 6`th of which differs per `n`.
    fn content(n: usize) -> Vec<u8> {
        (0..6)
            .map(|i| {
                if i == n % 6 {
                    format!("changed{n}")
                } else {
                    format!("line{i}")
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
            .into_bytes()
    }

    /// A chain of `count` changesets, oldest first, each carried by
    /// `commit(n)`: an add of `a`, then repeating modify, rename `a` to `b`,
    /// modify, rename `b` to `a` and modify.
    fn chain(count: usize) -> (Vec<(String, Changeset)>, Blobs) {
        let mut out: Vec<(String, Changeset)> = Vec::new();
        let mut blobs = Blobs::new();
        let mut last: HashMap<&str, String> = HashMap::new();
        for n in 0..count {
            let digest = format!("{:064x}", n + 1);
            blobs.insert(digest.clone(), content(n));
            let d = Some(digest.as_str());
            let paths = match n % 5 {
                0 if n == 0 => {
                    last.insert("a", digest.clone());
                    vec![changed("a", PathOp::Add, None, d)]
                }
                0 | 1 => {
                    let old = last.insert("a", digest.clone());
                    vec![changed("a", PathOp::Modify, old.as_deref(), d)]
                }
                2 => {
                    let old = last.remove("a");
                    last.insert("b", digest.clone());
                    vec![
                        changed("b", PathOp::Rename, old.as_deref(), d),
                        changed("a", PathOp::Delete, old.as_deref(), None),
                    ]
                }
                3 => {
                    let old = last.insert("b", digest.clone());
                    vec![changed("b", PathOp::Modify, old.as_deref(), d)]
                }
                _ => {
                    let old = last.remove("b");
                    last.insert("a", digest.clone());
                    vec![
                        changed("a", PathOp::Rename, old.as_deref(), d),
                        changed("b", PathOp::Delete, old.as_deref(), None),
                    ]
                }
            };
            let parent = out.last().map(|(id, _)| id.clone()).unwrap_or_default();
            out.push((commit(n), changeset(n, &parent, paths)));
        }
        (out, blobs)
    }

    fn mem_of(chain: &[(String, Changeset)], blobs: &Blobs) -> Mem {
        Mem {
            rows: rebuild(chain, &blob_reader(blobs)).unwrap().into_iter().collect(),
            changesets: chain.iter().cloned().collect(),
        }
    }

    // spec: MEDULLA-SCM-R005
    #[test]
    fn incremental_rows_equal_rebuild() {
        let (chain, blobs) = chain(200);
        let read = blob_reader(&blobs);
        let mut rows: BTreeMap<String, Vec<u8>> = BTreeMap::new();
        let mut touched: HashMap<String, u64> = HashMap::new();
        let mut head: Option<HistoryHead> = None;
        for (commit_id, changeset) in &chain {
            let applied = {
                let prior = |path: &str| -> Result<Option<BlameEntry>, String> {
                    let Some(seq) = touched.get(path) else {
                        return Ok(None);
                    };
                    rows.get(&blame_key(path, *seq))
                        .map(|bytes| decode_blame(bytes))
                        .transpose()
                };
                apply_changeset(
                    head.as_ref(),
                    commit_id,
                    changeset,
                    &BlameInputs {
                        blob: &read,
                        prior: &prior,
                    },
                )
                .unwrap()
            };
            for (key, value) in applied {
                if key == head_key() {
                    head = Some(decode_head(&value).unwrap());
                }
                rows.insert(key, value);
            }
            let seq = head.as_ref().unwrap().seq;
            for changed in &changeset.changed_paths {
                touched.insert(changed.path.clone(), seq);
            }
        }
        let rebuilt: BTreeMap<String, Vec<u8>> =
            rebuild(&chain, &read).unwrap().into_iter().collect();
        assert_eq!(rows, rebuilt);
        assert_eq!(head.unwrap().seq, 200);
    }

    // spec: MEDULLA-SCM-R005.1
    #[test]
    fn path_history_lists_newest_first_up_to_rev() {
        let (chain, blobs) = chain(10);
        let mem = mem_of(&chain, &blobs);
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
        assert_eq!(messages, ["m6", "m5", "m4", "m2", "m1", "m0"]);
        let ops: Vec<PathOp> = all.entries.iter().map(|e| e.op).collect();
        assert_eq!(
            ops,
            [
                PathOp::Modify,
                PathOp::Modify,
                PathOp::Rename,
                PathOp::Delete,
                PathOp::Modify,
                PathOp::Add
            ]
        );
        assert_eq!(all.entries[0].commit, commit(6));
        let capped = path_history(&mem, &head, &request(6, 3)).unwrap();
        assert_eq!(capped.entries.len(), 3);
        assert_eq!(capped.entries[2].message, "m4");
        // A path no changeset touched has no history.
        let mut untouched = request(6, 0);
        untouched.path = "zzz".into();
        assert!(path_history(&mem, &head, &untouched)
            .unwrap()
            .entries
            .is_empty());
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

    // spec: MEDULLA-SCM-R005.2
    #[test]
    fn blame_keeps_origins_across_modify_and_rename() {
        let mut blobs = Blobs::new();
        for (digest, text) in [
            ("x0", "A\nB\nC\nD\nE\n"),
            ("x1", "A\nX\nY\nD\nE\n"),
            ("x2", "A\nX\nY\nD\nZ\n"),
        ] {
            blobs.insert(digest.to_string(), text.as_bytes().to_vec());
        }
        blobs.insert("bin".to_string(), vec![0, 1, 2, 0]);
        let steps = vec![
            vec![changed("f", PathOp::Add, None, Some("x0"))],
            vec![changed("f", PathOp::Modify, Some("x0"), Some("x1"))],
            vec![
                changed("g", PathOp::Rename, Some("x1"), Some("x2")),
                changed("f", PathOp::Delete, Some("x1"), None),
            ],
            vec![changed("h", PathOp::Add, None, Some("bin"))],
        ];
        let mut chain: Vec<(String, Changeset)> = Vec::new();
        for (n, paths) in steps.into_iter().enumerate() {
            let parent = chain.last().map(|(id, _)| id.clone()).unwrap_or_default();
            chain.push((commit(n), changeset(n, &parent, paths)));
        }
        let mem = mem_of(&chain, &blobs);
        let head = commit(3);
        let request = |n: usize, path: &str| ReadBlameRequest {
            repo: "r".into(),
            branch: "b".into(),
            rev: commit(n),
            path: path.into(),
        };
        let id = |n: usize| chain[n].1.id.clone();
        let run = |start: u32, count: u32, n: usize| BlameRun {
            start_line: start,
            line_count: count,
            commit: commit(n),
            changeset_id: id(n),
        };
        // After the modify, lines 2 and 3 belong to the second changeset.
        let at_modify = blame(&mem, &head, &request(1, "f")).unwrap();
        assert_eq!(at_modify.runs, [run(0, 1, 0), run(1, 2, 1), run(3, 2, 0)]);
        // The rename carries the origins; the replaced last line is new.
        let at_rename = blame(&mem, &head, &request(2, "g")).unwrap();
        assert_eq!(
            at_rename.runs,
            [run(0, 1, 0), run(1, 2, 1), run(3, 1, 0), run(4, 1, 2)]
        );
        // The renamed-away path is absent; a binary file is refused.
        assert!(matches!(
            blame(&mem, &head, &request(2, "f")),
            Err(HistoryError::Refused(r)) if r.refusal == ChangesetRefusal::BlamePathAbsent
        ));
        assert!(matches!(
            blame(&mem, &head, &request(3, "h")),
            Err(HistoryError::Refused(r)) if r.refusal == ChangesetRefusal::BlameBinary
        ));
        assert!(matches!(
            blame(&mem, &head, &request(3, "nope")),
            Err(HistoryError::Refused(r)) if r.refusal == ChangesetRefusal::BlamePathAbsent
        ));
    }

    // spec: MEDULLA-SCM-R005.2
    #[test]
    fn a_file_over_the_line_bound_is_too_large() {
        let mut blobs = Blobs::new();
        let big = "l\n".repeat(BLAME_MAX_LINES + 1);
        blobs.insert("big".to_string(), big.into_bytes());
        let chain = vec![(
            commit(0),
            changeset(0, "", vec![changed("big", PathOp::Add, None, Some("big"))]),
        )];
        let mem = mem_of(&chain, &blobs);
        let request = ReadBlameRequest {
            repo: "r".into(),
            branch: "b".into(),
            rev: commit(0),
            path: "big".into(),
        };
        assert!(matches!(
            blame(&mem, &commit(0), &request),
            Err(HistoryError::Refused(r)) if r.refusal == ChangesetRefusal::BlameFileTooLarge
        ));
    }
}
