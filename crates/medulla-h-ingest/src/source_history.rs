//! The history index written with an accepted changeset and the path-history
//! read (MEDULLA-SCM-R005, MEDULLA-SCM-R005.1).
//!
//! `index_rows` is the write side: it returns the rows that commit in the same
//! registry transaction as the changeset payload row, so no index row lags or
//! leads the chain. When the head row is absent or of another index version
//! the whole chain is rebuilt first. `read_path_history` is the read side over
//! `medulla_compute::source_history::path_history`.

use medulla_compute::source_history::{
    apply_changeset, decode_head, head_key, path_history, rebuild, HistoryError, HistoryIndex,
    HistoryRow, SOURCE_HISTORY_INDEX_VERSION,
};
use medulla_source_types::source_changeset::{
    Changeset, ReadPathHistoryReceipt, ReadPathHistoryRequest, SubmitRefusal,
};
use std::collections::BTreeMap;

use crate::source_changeset::ChainView;

/// Reads one history row by relative key inside the append transaction.
pub type HistoryRowReader<'a> = &'a dyn Fn(&str) -> Result<Option<Vec<u8>>, String>;

/// The index rows that commit with `changeset`, the changeset carried by the
/// new branch commit `commit_id` that extends the chain `chain`.
///
/// The rows are the rebuild of the whole existing chain when the head row is
/// absent or of another version, followed by the rows of the new changeset.
pub fn index_rows(
    history: HistoryRowReader<'_>,
    chain: &dyn ChainView,
    changeset: &Changeset,
    commit_id: &str,
) -> Result<Vec<HistoryRow>, String> {
    let stored = match history(&head_key())? {
        Some(bytes) => Some(decode_head(&bytes)?),
        None => None,
    }
    .filter(|head| head.version == SOURCE_HISTORY_INDEX_VERSION);
    let mut rows: BTreeMap<String, Vec<u8>> = BTreeMap::new();
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
            let rebuilt = rebuild(&newest_first)?;
            let head = rebuilt
                .iter()
                .find(|(key, _)| *key == head_key())
                .map(|(_, bytes)| decode_head(bytes))
                .transpose()?;
            rows.extend(rebuilt);
            head
        }
    };
    rows.extend(apply_changeset(prev.as_ref(), commit_id, changeset)?);
    Ok(rows.into_iter().collect())
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
    match path_history(index, branch_head, request) {
        Ok(receipt) => Ok(Ok(receipt)),
        Err(HistoryError::Refused(refusal)) => Ok(Err(refusal)),
        Err(HistoryError::Store(error)) => Err(error),
    }
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
        let fresh = changeset(3, &mem.head);
        let rows: BTreeMap<String, Vec<u8>> = index_rows(&none, &mem, &fresh, "c3")
            .unwrap()
            .into_iter()
            .collect();
        let mut chain: Vec<(String, Changeset)> = (0..3)
            .map(|n| (format!("c{n}"), mem.changesets[&format!("c{n}")].clone()))
            .collect();
        chain.push(("c3".to_string(), fresh));
        let expected: BTreeMap<String, Vec<u8>> = rebuild(&chain).unwrap().into_iter().collect();
        assert_eq!(rows, expected);
    }
}
