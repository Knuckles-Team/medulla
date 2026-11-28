//! `ReadTree` handler and the checkpoint written with an accepted changeset
//! (MEDULLA-SCM-R004).
//!
//! `read_tree` validates the request and answers from
//! `medulla_compute::source_tree::tree_at`, which replays the chain from the
//! nearest stored checkpoint. `checkpoint_after` is the write side: when the
//! changeset being appended makes the chain a multiple of
//! `tree_checkpoint_interval` long, it returns the full tree snapshot that
//! commits with that changeset.

use medulla_source_types::source_changeset::{
    Changeset, ChangesetRefusal, ReadTreeReceipt, ReadTreeRequest, SubmitRefusal,
};
use {
    medulla_compute_assemble::source_tree::checkpoint_due,
    medulla_compute_assemble::source_tree::encode_checkpoint,
    medulla_compute_assemble::source_tree::replay, medulla_compute_assemble::source_tree::tree_at,
    medulla_compute_assemble::source_tree::TreeChain,
    medulla_compute_assemble::source_tree::TreeError,
};

use crate::source_changeset::ChainView;

/// Serve one `ReadTree`: the files under the prefix at `rev`.
///
/// Refusals: `CHANGESET_INVALID` for a request out of bounds and
/// `REVISION_NOT_ON_CHAIN` for a `rev` that is not a changeset of the chain.
/// A store failure is returned as `Err(text)` in the outer result.
pub fn read_tree(
    chain: &dyn TreeChain,
    request: &ReadTreeRequest,
) -> Result<Result<ReadTreeReceipt, SubmitRefusal>, String> {
    let prefix = match request.validate() {
        Ok(prefix) => prefix,
        Err(refusal) => return Ok(Err(refusal)),
    };
    match tree_at(chain, &request.rev, &prefix, request.depth) {
        Ok(entries) => Ok(Ok(ReadTreeReceipt {
            head_revision: request.rev.clone(),
            entries,
        })),
        Err(TreeError::NotOnChain) => Ok(Err(SubmitRefusal::new(
            ChangesetRefusal::RevisionNotOnChain,
            "rev is not a changeset of the branch",
        ))),
        Err(TreeError::Store(error)) => Err(error),
    }
}

/// The encoded tree snapshot that commits with `appended`, the changeset
/// extending the chain whose head is `chain.head()`, or `None` when the new
/// chain length is not a multiple of `interval`.
pub fn checkpoint_after(
    chain: &dyn ChainView,
    appended: &Changeset,
    interval: u64,
) -> Result<Option<Vec<u8>>, String> {
    let mut newest_first: Vec<Changeset> = Vec::new();
    let mut cursor = chain.head().to_string();
    while let Some(found) = chain.changeset_at(&cursor)? {
        cursor = found.parents.first().cloned().unwrap_or_default();
        newest_first.push(found);
    }
    if !checkpoint_due(newest_first.len() as u64 + 1, interval) {
        return Ok(None);
    }
    let mut history: Vec<Changeset> = newest_first.into_iter().rev().collect();
    history.push(appended.clone());
    encode_checkpoint(&replay(&history)).map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_compute_assemble::source_tree::TreeChain;
    use medulla_source_types::source_changeset::{ChangedPath, PathOp, Trailers};
    use std::collections::HashMap;

    struct Mem {
        changesets: HashMap<String, Changeset>,
        head: String,
    }

    impl ChainView for Mem {
        fn head(&self) -> &str {
            &self.head
        }
        fn changeset_at(&self, commit_id: &str) -> Result<Option<Changeset>, String> {
            Ok(self.changesets.get(commit_id).cloned())
        }
    }

    impl TreeChain for Mem {
        fn changeset_at(&self, rev: &str) -> Result<Option<Changeset>, String> {
            Ok(self.changesets.get(rev).cloned())
        }
        fn checkpoint_at(&self, _rev: &str) -> Result<Option<Vec<u8>>, String> {
            Ok(None)
        }
    }

    fn next(mem: &mut Mem, n: usize) -> Changeset {
        Changeset {
            id: String::new(),
            repo: "r".into(),
            parents: vec![mem.head.clone()],
            author_principal: "p".into(),
            message: format!("m{n}"),
            changed_paths: vec![ChangedPath {
                path: format!("f{n}"),
                op: PathOp::Add,
                old_blob: None,
                new_blob: Some(format!("{n:064}")),
                mode: 0o100644,
            }],
            created_at: n as u64,
            trailers: Trailers::default(),
            agent_identity: None,
            run_trace_id: None,
            git: None,
            symbol_edit: None,
        }
        .sealed()
    }

    // spec: MEDULLA-SCM-R004
    #[test]
    fn checkpoint_is_written_at_the_interval_and_reads_back() {
        let mut mem = Mem {
            changesets: HashMap::new(),
            head: "base".into(),
        };
        let mut checkpoints = 0;
        for n in 1..=20usize {
            let changeset = next(&mut mem, n);
            let snapshot = checkpoint_after(&mem, &changeset, 10).unwrap();
            assert_eq!(snapshot.is_some(), n % 10 == 0, "changeset {n}");
            checkpoints += usize::from(snapshot.is_some());
            mem.head = changeset.id.clone();
            mem.changesets.insert(changeset.id.clone(), changeset);
        }
        assert_eq!(checkpoints, 2);
        let request = ReadTreeRequest {
            repo: "r".into(),
            branch: "b".into(),
            rev: mem.head.clone(),
            prefix: String::new(),
            depth: 0,
        };
        let receipt = read_tree(&mem, &request).unwrap().unwrap();
        assert_eq!(receipt.entries.len(), 20);
        assert_eq!(
            receipt.head_revision, mem.head,
            "the receipt names the commit the tree was read at"
        );
        let mut missing = request.clone();
        missing.rev = "0".repeat(64);
        let refusal = read_tree(&mem, &missing).unwrap().unwrap_err();
        assert_eq!(refusal.refusal, ChangesetRefusal::RevisionNotOnChain);
        let mut deep = request;
        deep.depth = 65;
        assert_eq!(
            read_tree(&mem, &deep).unwrap().unwrap_err().refusal,
            ChangesetRefusal::ChangesetInvalid
        );
    }
}
