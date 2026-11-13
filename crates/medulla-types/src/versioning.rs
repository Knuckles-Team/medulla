//! Transaction, commit and diff vocabulary of branch time travel
//! (MEDULLA-BRANCH-MERGE-R006).
//!
//! A branch has ONE commit chain (R002). Every version on that chain reads
//! (`BranchReadAt`) and diffs (`BranchDiff`). The vocabulary here is pure
//! serde: [`KgCommit`] is the chain's commit, [`KgTransaction`] is one commit
//! with the entity changes it carries, and [`KgDiff`] is the typed change feed
//! between two versions of the chain, one page at a time.

use serde::{Deserialize, Serialize};

use crate::branch::BranchCommit;

/// A commit on the single branch chain; the same record R002 appends.
pub type KgCommit = BranchCommit;

/// What one change did to its entity, against the state before it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum DiffOperation {
    Added,
    Updated,
    Deleted,
}

/// The entity one change touches. `Other` names a graph mutation whose
/// entities this feed does not enumerate, so no change is ever hidden.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DiffEntity {
    /// A node, by node id.
    Node { id: String },
    /// An edge, by `(source, target)`.
    Edge { source: String, target: String },
    /// A file path of a source changeset (MEDULLA-SCM-R002).
    Path { path: String },
    /// A graph mutation the feed does not enumerate, by method name.
    Other { method: String },
}

/// One entry of the change feed: entity, operation and the image ref of the
/// commit that carries the change (`commit:<id>`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct KgDiffEntry {
    pub entity: DiffEntity,
    pub operation: DiffOperation,
    pub image_ref: String,
}

/// One commit with the entity changes it carries, in apply order.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct KgTransaction {
    pub commit: KgCommit,
    pub changes: Vec<KgDiffEntry>,
}

/// The change feed between two versions of one branch chain: the changes of
/// every commit after `from_version` up to and including `to_version`. One
/// page holds at most `branch.diff_page_size` entries; `next_offset` names
/// the first entry of the next page, absent on the last page.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct KgDiff {
    pub branch: String,
    /// `None` is the empty baseline before the first commit.
    pub from_version: Option<u64>,
    pub to_version: u64,
    /// The commits the feed spans, oldest first.
    pub commits: Vec<KgCommit>,
    pub entries: Vec<KgDiffEntry>,
    pub next_offset: Option<u64>,
}

/// One node of a version snapshot.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct SnapshotNode {
    pub id: String,
    #[cfg_attr(feature = "contract-schema", schemars(with = "Vec<u8>"))]
    #[serde(with = "serde_bytes")]
    pub properties_msgpack: Vec<u8>,
}

/// One edge of a version snapshot.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct SnapshotEdge {
    pub source: String,
    pub target: String,
    #[cfg_attr(feature = "contract-schema", schemars(with = "Vec<u8>"))]
    #[serde(with = "serde_bytes")]
    pub properties_msgpack: Vec<u8>,
}

/// The state of a branch as of one version on its chain.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct BranchSnapshot {
    pub branch: String,
    pub engine_version: u64,
    /// The chain commit that produced `engine_version`.
    pub commit: KgCommit,
    /// Nodes in id order.
    pub nodes: Vec<SnapshotNode>,
    /// Edges in `(source, target)` order.
    pub edges: Vec<SnapshotEdge>,
}

/// The page of `entries` that starts at `offset`: at most `page_size`
/// entries and the offset of the next page, `None` on the last.
pub fn page_of<T: Clone>(entries: &[T], offset: u64, page_size: u64) -> (Vec<T>, Option<u64>) {
    let start = usize::try_from(offset)
        .unwrap_or(usize::MAX)
        .min(entries.len());
    let size = usize::try_from(page_size).unwrap_or(usize::MAX).max(1);
    let end = start.saturating_add(size).min(entries.len());
    let next = (end < entries.len()).then_some(end as u64);
    (entries[start..end].to_vec(), next)
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-BRANCH-MERGE-R006
    #[test]
    fn pages_are_bounded_and_chain_by_offset() {
        let entries: Vec<u32> = (0..5).collect();
        assert_eq!(page_of(&entries, 0, 2), (vec![0, 1], Some(2)));
        assert_eq!(page_of(&entries, 2, 2), (vec![2, 3], Some(4)));
        assert_eq!(page_of(&entries, 4, 2), (vec![4], None));
        assert_eq!(page_of(&entries, 9, 2), (vec![], None));
        assert_eq!(page_of(&entries, 0, 0).0.len(), 1);
    }

    // spec: MEDULLA-BRANCH-MERGE-R006
    #[test]
    fn diff_entity_wire_form_is_tagged_and_round_trips() {
        let entry = KgDiffEntry {
            entity: DiffEntity::Edge {
                source: "a".into(),
                target: "b".into(),
            },
            operation: DiffOperation::Deleted,
            image_ref: "commit:c1".into(),
        };
        let bytes = rmp_serde::to_vec_named(&entry).unwrap();
        assert_eq!(rmp_serde::from_slice::<KgDiffEntry>(&bytes).unwrap(), entry);
        let json = serde_json::to_string(&entry).unwrap();
        assert!(json.contains("\"kind\":\"edge\"") && json.contains("\"deleted\""));
    }
}
