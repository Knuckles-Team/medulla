//! The history index written with an accepted changeset and the path-history
//! read (MEDULLA-SCM-R005, MEDULLA-SCM-R005.1).
//!
//! `index_rows` is the write side: it returns the rows that commit in the same
//! registry transaction as the changeset payload row, so no index row lags or
//! leads the chain. When the head row is absent or of another index version
//! the whole chain is rebuilt first. `read_path_history` is the read side over
//! `medulla_compute::source_history::path_history`.

use medulla_compute::source_history::{
    apply_changeset, blame, blame_key, decode_blame, decode_head, decode_seq, head_key,
    path_history, prior_blame_paths, rebuild, seq_key, BlameEntry, BlameInputs, HistoryError,
    HistoryIndex, HistoryRow, SOURCE_HISTORY_INDEX_VERSION,
};
use medulla_source_types::source_changeset::{
    Changeset, ReadBlameReceipt, ReadBlameRequest, ReadPathHistoryReceipt,
    ReadPathHistoryRequest, SubmitRefusal,
};
use std::collections::{BTreeMap, HashMap, HashSet};

use crate::source_changeset::{BlobIo, ChainView};

/// Reads one history row by relative key inside the append transaction.
pub type HistoryRowReader<'a> = &'a dyn Fn(&str) -> Result<Option<Vec<u8>>, String>;

/// The commit that last changed each of `wanted`, from a walk of `chain` from
/// its head that stops once every path is found.
fn newest_touch(
    chain: &dyn ChainView,
    wanted: &HashSet<String>,
) -> Result<HashMap<String, String>, String> {
    let mut found: HashMap<String, String> = HashMap::new();
    let mut cursor = chain.head().to_string();
    while found.len() < wanted.len() {
        let Some(changeset) = chain.changeset_at(&cursor)? else {
            break;
        };
        for changed in &changeset.changed_paths {
            if wanted.contains(&changed.path) && !found.contains_key(&changed.path) {
                found.insert(changed.path.clone(), cursor.clone());
            }
        }
        cursor = changeset.parents.first().cloned().unwrap_or_default();
    }
    Ok(found)
}

/// The index rows that commit with `changeset`, the changeset carried by the
/// new branch commit `commit_id` that extends the chain `chain`.
///
/// The rows are the rebuild of the whole existing chain when the head row is
/// absent or of another version, followed by the rows of the new changeset.
/// `blobs` supplies the file bytes the blame rows are computed from.
pub fn index_rows(
    history: HistoryRowReader<'_>,
    chain: &dyn ChainView,
    changeset: &Changeset,
    commit_id: &str,
    blobs: &dyn BlobIo,
) -> Result<Vec<HistoryRow>, String> {
    let stored = match history(&head_key())? {
        Some(bytes) => Some(decode_head(&bytes)?),
        None => None,
    }
    .filter(|head| head.version == SOURCE_HISTORY_INDEX_VERSION);
    let read_blob = |digest: &str| blobs.read(digest);
    let mut rows: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    let rebuilt = stored.is_none();
    let prev = match stored {
        Some(head) => Some(head),
        None => {
            let mut newest_first: Vec<(String, Changeset)> = Vec::new();
            let mut cursor = chain.head().to_string();
            while let Some(found) = chain.changeset_at(&cursor)? {
                let next = found.parents.first().cloned().unwrap_or_default();
                newest_first.push((cursor, found));
                cursor = next;
            }
            newest_first.reverse();
            let whole = rebuild(&newest_first, &read_blob)?;
            let head = whole
                .iter()
                .find(|(key, _)| *key == head_key())
                .map(|(_, bytes)| decode_head(bytes))
                .transpose()?;
            rows.extend(whole);
            head
        }
    };
    let wanted: HashSet<String> = prior_blame_paths(changeset).into_iter().collect();
    let touched = newest_touch(chain, &wanted)?;
    let lookup = |key: &str| -> Result<Option<Vec<u8>>, String> {
        match rows.get(key) {
            Some(value) => Ok(Some(value.clone())),
            None if rebuilt => Ok(None),
            None => history(key),
        }
    };
    let prior = |path: &str| -> Result<Option<BlameEntry>, String> {
        let Some(commit) = touched.get(path) else {
            return Ok(None);
        };
        let Some(seq) = lookup(&seq_key(commit))? else {
            return Ok(None);
        };
        lookup(&blame_key(path, decode_seq(&seq)?))?
            .map(|bytes| decode_blame(&bytes))
            .transpose()
    };
    let added = apply_changeset(
        prev.as_ref(),
        commit_id,
        changeset,
        &BlameInputs {
            blob: &read_blob,
            prior: &prior,
        },
    )?;
    let mut merged = rows.clone();
    merged.extend(added);
    Ok(merged.into_iter().collect())
}

fn refusal_or_store<T>(outcome: Result<T, HistoryError>) -> Result<Result<T, SubmitRefusal>, String> {
    match outcome {
        Ok(receipt) => Ok(Ok(receipt)),
        Err(HistoryError::Refused(refusal)) => Ok(Err(refusal)),
        Err(HistoryError::Store(error)) => Err(error),
    }
}

/// Serve one `ReadPathHistory`: the changesets that touched the path at or
/// below `rev`, newest first.
///
/// Refusals: `CHANGESET_INVALID`, `REVISION_NOT_ON_CHAIN` and
/// `HISTORY_INDEX_STALE`. A store failure is returned as `Err(text)`.
pub fn read_path_history(
    index: &dyn HistoryIndex,
    branch_head: &str,
    request: &ReadPathHistoryRequest,
) -> Result<Result<ReadPathHistoryReceipt, SubmitRefusal>, String> {
    refusal_or_store(path_history(index, branch_head, request))
}

/// Serve one `ReadBlame`: the origin changeset of every line of the path at
/// `rev`.
///
/// Refusals: those of `read_path_history` plus `BLAME_BINARY`,
/// `BLAME_FILE_TOO_LARGE` and `BLAME_PATH_ABSENT`.
pub fn read_blame(
    index: &dyn HistoryIndex,
    branch_head: &str,
    request: &ReadBlameRequest,
) -> Result<Result<ReadBlameReceipt, SubmitRefusal>, String> {
    refusal_or_store(blame(index, branch_head, request))
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_source_types::source_changeset::{ChangedPath, PathOp, Trailers};
    use std::collections::HashMap;

    struct Mem {
        head: String,
        changesets: HashMap<String, Changeset>,
    }

    impl ChainView for Mem {
        fn head(&self) -> &str {
            &self.head
        }
        fn changeset_at(&self, commit_id: &str) -> Result<Option<Changeset>, String> {
            Ok(self.changesets.get(commit_id).cloned())
        }
    }

    struct Blobs;

    impl BlobIo for Blobs {
        fn read(&self, _digest: &str) -> Result<Vec<u8>, String> {
            Ok(b"x\n".to_vec())
        }
        fn store(
            &self,
            _bytes: &[u8],
        ) -> Result<medulla_source_types::source_changeset::SubmittedBlobRef, String> {
            Err("not used".to_string())
        }
    }

    fn changeset(n: usize, parent: &str) -> Changeset {
        Changeset {
            id: String::new(),
            repo: "r".into(),
            parents: vec![parent.to_string()],
            author_principal: "p".into(),
            message: format!("m{n}"),
            changed_paths: vec![ChangedPath {
                path: format!("f{n}"),
                op: PathOp::Add,
                old_blob: None,
                new_blob: Some("b".repeat(64)),
                mode: 0o100644,
            }],
            created_at: n as u64,
            trailers: Trailers::default(),
            agent_identity: None,
            run_trace_id: None,
        }
        .sealed()
    }

    // spec: MEDULLA-SCM-R005.1
    #[test]
    fn an_absent_head_row_rebuilds_the_whole_chain_before_the_new_rows() {
        let mut mem = Mem {
            head: "base".into(),
            changesets: HashMap::new(),
        };
        for n in 0..3 {
            let next = changeset(n, &mem.head);
            let id = format!("c{n}");
            mem.changesets.insert(id.clone(), next);
            mem.head = id;
        }
        let none = |_: &str| -> Result<Option<Vec<u8>>, String> { Ok(None) };
        let blobs = Blobs;
        let fresh = changeset(3, &mem.head);
        let rows: BTreeMap<String, Vec<u8>> = index_rows(&none, &mem, &fresh, "c3", &blobs)
            .unwrap()
            .into_iter()
            .collect();
        let mut chain: Vec<(String, Changeset)> = (0..3)
            .map(|n| (format!("c{n}"), mem.changesets[&format!("c{n}")].clone()))
            .collect();
        chain.push(("c3".to_string(), fresh));
        let expected: BTreeMap<String, Vec<u8>> = rebuild(&chain, &|_: &str| Ok(b"x\n".to_vec())).unwrap().into_iter().collect();
        assert_eq!(rows, expected);
    }
}
