//! Tree materialization of the source-changeset chain (MEDULLA-SCM-R004).
//!
//! The tree of a repository at one revision is the replay of the chain's
//! changesets from the nearest stored checkpoint, a full tree snapshot written
//! every `SchedulerConfig.tree_checkpoint_interval` changesets. A replay applies
//! only the paths under the requested prefix, so a subtree read touches the
//! subtree plus the changesets between the checkpoint and the revision, never
//! the whole history.

use std::collections::BTreeMap;

use medulla_source_types::source_changeset::{Changeset, PathOp, TreeEntry};

/// The files of a tree: path to `(blob digest, mode)`, ordered by path.
pub type Tree = BTreeMap<String, (String, u32)>;

/// The chain as a tree read sees it.
pub trait TreeChain {
    /// The changeset the commit `rev` carries, if it carries one.
    fn changeset_at(&self, rev: &str) -> Result<Option<Changeset>, String>;
    /// The encoded full-tree snapshot stored at `rev`, if one was written.
    fn checkpoint_at(&self, rev: &str) -> Result<Option<Vec<u8>>, String>;
}

/// Why a tree read produced no entries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TreeError {
    /// `rev` is not a changeset of the chain (`REVISION_NOT_ON_CHAIN`).
    NotOnChain,
    /// The chain store failed or a snapshot did not decode.
    Store(String),
}

/// Whether `path` lies under `prefix` on a path-segment boundary. The empty
/// prefix holds every path.
pub fn in_prefix(path: &str, prefix: &str) -> bool {
    prefix.is_empty()
        || path == prefix
        || path
            .strip_prefix(prefix)
            .is_some_and(|rest| rest.starts_with('/'))
}

/// Apply one changeset to `tree`, touching only the paths under `prefix`.
/// A delete removes the path; every other op sets the path to its new blob.
pub fn apply_scoped(tree: &mut Tree, changeset: &Changeset, prefix: &str) {
    for changed in &changeset.changed_paths {
        if !in_prefix(&changed.path, prefix) {
            continue;
        }
        match (&changed.op, &changed.new_blob) {
            (PathOp::Delete, _) | (_, None) => {
                tree.remove(&changed.path);
            }
            (_, Some(digest)) => {
                tree.insert(changed.path.clone(), (digest.clone(), changed.mode));
            }
        }
    }
}

/// Encode a full tree as the checkpoint record.
pub fn encode_checkpoint(tree: &Tree) -> Result<Vec<u8>, String> {
    let entries: Vec<TreeEntry> = tree
        .iter()
        .map(|(path, (digest, mode))| TreeEntry {
            path: path.clone(),
            blob_digest: digest.clone(),
            mode: *mode,
        })
        .collect();
    rmp_serde::to_vec_named(&entries).map_err(|error| error.to_string())
}

fn decode_checkpoint(bytes: &[u8], prefix: &str) -> Result<Tree, String> {
    let entries: Vec<TreeEntry> = rmp_serde::from_slice(bytes).map_err(|e| e.to_string())?;
    Ok(entries
        .into_iter()
        .filter(|entry| in_prefix(&entry.path, prefix))
        .map(|entry| (entry.path, (entry.blob_digest, entry.mode)))
        .collect())
}

/// Whether the changeset that makes the chain `count` changesets long carries
/// a checkpoint.
pub fn checkpoint_due(count: u64, interval: u64) -> bool {
    interval > 0 && count > 0 && count % interval == 0
}

/// The full tree after `history` (oldest first) has been replayed.
pub fn replay(history: &[Changeset]) -> Tree {
    let mut tree = Tree::new();
    for changeset in history {
        apply_scoped(&mut tree, changeset, "");
    }
    tree
}

fn depth_ok(path: &str, prefix: &str, depth: u32) -> bool {
    if depth == 0 {
        return true;
    }
    let rest = path.strip_prefix(prefix).unwrap_or(path);
    let segments = rest.split('/').filter(|s| !s.is_empty()).count();
    segments <= depth as usize
}

fn walk(
    chain: &dyn TreeChain,
    rev: &str,
    prefix: &str,
    use_checkpoints: bool,
) -> Result<Tree, TreeError> {
    let store = TreeError::Store;
    if chain.changeset_at(rev).map_err(store)?.is_none() {
        return Err(TreeError::NotOnChain);
    }
    let mut pending: Vec<Changeset> = Vec::new();
    let mut tree = Tree::new();
    let mut cursor = rev.to_string();
    loop {
        if use_checkpoints {
            if let Some(bytes) = chain.checkpoint_at(&cursor).map_err(TreeError::Store)? {
                tree = decode_checkpoint(&bytes, prefix).map_err(TreeError::Store)?;
                break;
            }
        }
        let Some(changeset) = chain.changeset_at(&cursor).map_err(TreeError::Store)? else {
            break;
        };
        cursor = changeset.parents.first().cloned().unwrap_or_default();
        pending.push(changeset);
    }
    for changeset in pending.iter().rev() {
        apply_scoped(&mut tree, changeset, prefix);
    }
    Ok(tree)
}

fn entries(tree: Tree, prefix: &str, depth: u32) -> Vec<TreeEntry> {
    tree.into_iter()
        .filter(|(path, _)| depth_ok(path, prefix, depth))
        .map(|(path, (blob_digest, mode))| TreeEntry {
            path,
            blob_digest,
            mode,
        })
        .collect()
}

/// The files under `prefix` at `rev`, at most `depth` path segments below the
/// prefix (`0` is unbounded), replayed from the nearest checkpoint at or below
/// `rev`. `prefix` is without a trailing separator; empty means the whole tree.
pub fn tree_at(
    chain: &dyn TreeChain,
    rev: &str,
    prefix: &str,
    depth: u32,
) -> Result<Vec<TreeEntry>, TreeError> {
    Ok(entries(walk(chain, rev, prefix, true)?, prefix, depth))
}

/// The same answer as [`tree_at`] from a replay of the whole history, ignoring
/// every checkpoint.
pub fn tree_at_full_replay(
    chain: &dyn TreeChain,
    rev: &str,
    prefix: &str,
    depth: u32,
) -> Result<Vec<TreeEntry>, TreeError> {
    Ok(entries(walk(chain, rev, prefix, false)?, prefix, depth))
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_source_types::source_changeset::{ChangedPath, Trailers};
    use std::collections::HashMap;

    struct Mem {
        changesets: HashMap<String, Changeset>,
        checkpoints: HashMap<String, Vec<u8>>,
    }

    impl TreeChain for Mem {
        fn changeset_at(&self, rev: &str) -> Result<Option<Changeset>, String> {
            Ok(self.changesets.get(rev).cloned())
        }
        fn checkpoint_at(&self, rev: &str) -> Result<Option<Vec<u8>>, String> {
            Ok(self.checkpoints.get(rev).cloned())
        }
    }

    fn changed(path: &str, op: PathOp, blob: Option<&str>) -> ChangedPath {
        ChangedPath {
            path: path.to_string(),
            op,
            old_blob: None,
            new_blob: blob.map(str::to_string),
            mode: 0o100644,
        }
    }

    /// A ten-revision chain; every `interval`th revision carries a checkpoint.
    fn chain(interval: u64) -> (Mem, Vec<String>) {
        let mut mem = Mem {
            changesets: HashMap::new(),
            checkpoints: HashMap::new(),
        };
        let mut ids: Vec<String> = Vec::new();
        let mut history: Vec<Changeset> = Vec::new();
        for n in 0..10u32 {
            let paths = match n {
                0 => vec![
                    changed("src/a.rs", PathOp::Add, Some("a0")),
                    changed("src/deep/b.rs", PathOp::Add, Some("b0")),
                    changed("README", PathOp::Add, Some("r0")),
                ],
                3 => vec![changed("src/a.rs", PathOp::Modify, Some("a3"))],
                5 => vec![changed("src/deep/b.rs", PathOp::Delete, None)],
                7 => vec![
                    changed("src/c.rs", PathOp::Rename, Some("a3")),
                    changed("src/a.rs", PathOp::Delete, None),
                ],
                _ => vec![changed(&format!("src/f{n}.rs"), PathOp::Add, Some("x"))],
            };
            let changeset = Changeset {
                id: String::new(),
                repo: "r".into(),
                parents: ids.last().cloned().into_iter().collect(),
                author_principal: "p".into(),
                message: format!("m{n}"),
                changed_paths: paths,
                created_at: n as u64,
                trailers: Trailers::default(),
                agent_identity: None,
                run_trace_id: None,
            }
            .sealed();
            let id = changeset.id.clone();
            history.push(changeset.clone());
            if checkpoint_due(history.len() as u64, interval) {
                mem.checkpoints
                    .insert(id.clone(), encode_checkpoint(&replay(&history)).unwrap());
            }
            mem.changesets.insert(id.clone(), changeset);
            ids.push(id);
        }
        (mem, ids)
    }

    /// Verifies MEDULLA-SCM-R004: a subtree read at each revision equals the
    /// full replay, with and without a checkpoint.
    // spec: MEDULLA-SCM-R004
    #[test]
    fn tree_at_revision_matches_full_replay() {
        let (plain, ids) = chain(1000);
        let (checkpointed, same_ids) = chain(4);
        assert_eq!(ids, same_ids);
        assert!(!checkpointed.checkpoints.is_empty());
        assert!(plain.checkpoints.is_empty());
        for rev in &ids {
            for (prefix, depth) in [("", 0), ("src", 0), ("src", 1), ("src/deep", 0), ("READ", 0)]
            {
                let full = tree_at_full_replay(&plain, rev, prefix, depth).unwrap();
                assert_eq!(tree_at(&plain, rev, prefix, depth).unwrap(), full);
                assert_eq!(tree_at(&checkpointed, rev, prefix, depth).unwrap(), full);
            }
        }
        let head = tree_at(&checkpointed, &ids[9], "", 0).unwrap();
        let paths: Vec<&str> = head.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(
            paths,
            [
                "README",
                "src/c.rs",
                "src/f1.rs",
                "src/f2.rs",
                "src/f4.rs",
                "src/f6.rs",
                "src/f8.rs",
                "src/f9.rs"
            ]
        );
        // A prefix returns only that prefix, on a segment boundary.
        let at3 = tree_at(&checkpointed, &ids[3], "src", 0).unwrap();
        assert!(at3.iter().all(|e| e.path.starts_with("src/")));
        assert!(tree_at(&checkpointed, &ids[3], "sr", 0).unwrap().is_empty());
        // Depth bounds the segments below the prefix.
        let shallow = tree_at(&checkpointed, &ids[0], "src", 1).unwrap();
        assert_eq!(shallow.len(), 1);
        assert_eq!(shallow[0].path, "src/a.rs");
        // A revision that is not on the chain is refused.
        assert_eq!(
            tree_at(&checkpointed, &"0".repeat(64), "", 0),
            Err(TreeError::NotOnChain)
        );
    }

    // spec: MEDULLA-SCM-R004
    #[test]
    fn checkpoint_cadence_follows_the_interval() {
        assert!(!checkpoint_due(0, 10));
        assert!(!checkpoint_due(9, 10));
        assert!(checkpoint_due(10, 10));
        assert!(checkpoint_due(20, 10));
        assert!(in_prefix("src/a.rs", "src"));
        assert!(!in_prefix("srcx/a.rs", "src"));
    }
}
