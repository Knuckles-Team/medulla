//! Memory snapshot rows on the request graph (MEDULLA-DURABLE-KERNEL-R085.3,
//! R085.4, R085.6): the node layout and the reads of it.
//!
//! Layout: head `memsnap:<doc_id>`, revision `memrev:<doc_id>@<revision>`,
//! retained candidate `memcand:<doc_id>@<n>` and acknowledgment
//! `memack:<doc_id>`. This is the lowest crate the graph handler and the decide
//! handler both depend on, so both read the rows through one set of helpers.
//! Writes are planned by the graph handler as a
//! [`SnapshotPlan`](medulla_mutation_types::memory_sync::SnapshotPlan) and
//! applied by [`apply_plan`], which the commit and durable replay share.

use crate::graph::GraphCore;
use medulla_mutation_types::memory_sync::{
    revision_node_id, DocId, MemorySnapshotHead, MemorySnapshotRead, SnapshotPlan,
};
use medulla_types::contract::Digest256;
use serde::{Deserialize, Serialize};

/// The node label of a memory snapshot head row.
pub const HEAD_LABEL: &str = "MemorySnapshot";
/// The node label of a memory snapshot revision row.
pub const REVISION_LABEL: &str = "MemoryRevision";
/// The node label of a retained candidate row.
pub const CANDIDATE_LABEL: &str = "MemoryCandidate";
/// The node label of an acknowledgment row.
pub const ACK_LABEL: &str = "MemoryAck";

/// A head or revision row: the head record and the bytes it stands for.
#[derive(Serialize, Deserialize)]
pub struct RevisionRow {
    /// The row label, [`HEAD_LABEL`] or [`REVISION_LABEL`].
    pub label: String,
    /// The head record this row stands for.
    pub head: MemorySnapshotHead,
    /// The document content at this revision.
    pub content: String,
    /// The engine clock at the commit that wrote this revision. The revision
    /// before it was superseded at this time (MEDULLA-DURABLE-KERNEL-R085.7).
    pub committed_at_ms: u64,
}

/// A retained candidate row.
#[derive(Serialize, Deserialize)]
pub struct CandidateRow {
    /// The row label, [`CANDIDATE_LABEL`].
    pub label: String,
    /// The revision the candidate was written against.
    pub base_revision: u64,
    /// The candidate content.
    pub content: String,
    /// The digest of the candidate content.
    pub content_digest: Digest256,
    /// Whether the conflict this candidate holds is resolved.
    pub resolved: bool,
    /// The commit time of the resolution; `None` while the conflict is open.
    #[serde(default)]
    pub resolved_at_ms: Option<u64>,
}

/// An acknowledgment row.
#[derive(Serialize, Deserialize)]
pub struct AckRow {
    /// The row label, [`ACK_LABEL`].
    pub label: String,
    /// The revision the holder acknowledged.
    pub revision: u64,
    /// The digest of the acknowledged content.
    pub content_digest: Digest256,
    /// The observation generation the acknowledgment was made at.
    pub observation_generation: u64,
}

/// The canonical property bytes of one row.
pub fn encode_row<T: Serialize>(row: &T) -> Vec<u8> {
    let value = serde_json::to_value(row).unwrap_or(serde_json::Value::Null);
    rmp_serde::to_vec_named(&value).unwrap_or_default()
}

/// One typed row decoded from the property bytes a plan write carries.
pub fn decode_row<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> Option<T> {
    let value: serde_json::Value = rmp_serde::from_slice(bytes).ok()?;
    serde_json::from_value(value).ok()
}

/// The node id of a document's head.
pub fn head_node_id(doc: &DocId) -> String {
    format!("memsnap:{}", doc.as_str())
}

/// The id prefix of a document's retained candidates.
pub fn candidate_prefix(doc: &DocId) -> String {
    format!("memcand:{}@", doc.as_str())
}

/// The node id of a document's acknowledgment.
pub fn ack_node_id(doc: &DocId) -> String {
    format!("memack:{}", doc.as_str())
}

/// One typed row of `id`, if it exists and decodes.
pub fn load_row<T: for<'de> Deserialize<'de>>(core: &GraphCore, id: &str) -> Option<T> {
    core.get_node_properties(id)
        .and_then(|bytes| decode_row(&bytes))
}

/// The accepted head of `doc`, if any.
pub fn load_head(core: &GraphCore, doc: &DocId) -> Option<MemorySnapshotHead> {
    load_row::<RevisionRow>(core, &head_node_id(doc)).map(|row| row.head)
}

/// The head (`revision` `None`) or a retained revision, with its bytes.
pub fn read(core: &GraphCore, doc: &DocId, revision: Option<u64>) -> Option<MemorySnapshotRead> {
    let id = match revision {
        None => head_node_id(doc),
        Some(revision) => revision_node_id(doc, revision),
    };
    load_row::<RevisionRow>(core, &id).map(|row| MemorySnapshotRead {
        head: row.head,
        content: row.content,
        search_state: None,
    })
}

/// Every head whose `doc_id` starts with `prefix`, in `doc_id` order.
pub fn list_heads(core: &GraphCore, prefix: &str) -> Vec<MemorySnapshotHead> {
    let mut heads: Vec<MemorySnapshotHead> = core
        .get_nodes_by_label(HEAD_LABEL, usize::MAX)
        .into_iter()
        .filter_map(|(_, bytes)| decode_row::<RevisionRow>(&bytes))
        .map(|row| row.head)
        .filter(|head| head.doc_id.as_str().starts_with(prefix))
        .collect();
    heads.sort_by(|a, b| a.doc_id.cmp(&b.doc_id));
    heads
}

/// The retained candidates of `doc`, with their numbers.
pub fn candidates(core: &GraphCore, doc: &DocId) -> Vec<(u64, CandidateRow)> {
    let prefix = candidate_prefix(doc);
    core.get_nodes_by_label(CANDIDATE_LABEL, usize::MAX)
        .into_iter()
        .filter_map(|(id, bytes)| {
            let n = id.strip_prefix(&prefix)?.parse::<u64>().ok()?;
            Some((n, decode_row::<CandidateRow>(&bytes)?))
        })
        .collect()
}

/// Which blobs of one document stay held and which release.
pub struct RetentionPlan {
    /// `(slot, content)` to keep held.
    pub held: Vec<(SnapshotSlot, String)>,
    /// `(slot, content digest hex)` to release.
    pub released: Vec<(SnapshotSlot, String)>,
}

/// Which stored content of a document a blob holder names: a numbered revision
/// or a numbered retained candidate (MEDULLA-DURABLE-KERNEL-R085.4). Revision
/// and candidate numbers are separate sequences, so the holder id says which.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SnapshotSlot {
    /// A numbered revision.
    Revision(u64),
    /// A numbered retained candidate.
    Candidate(u64),
}

impl SnapshotSlot {
    /// The blob holder id: `memory_snapshot:<doc>@<n>` for revision `n`,
    /// `memory_snapshot:<doc>@c<n>` for candidate `n`.
    pub fn holder_id(&self, doc: &DocId) -> String {
        match self {
            Self::Revision(n) => format!("memory_snapshot:{}@{n}", doc.as_str()),
            Self::Candidate(n) => format!("memory_snapshot:{}@c{n}", doc.as_str()),
        }
    }
}

/// The retention split of `doc` at `now_ms` (MEDULLA-DURABLE-KERNEL-R085.7). A
/// superseded revision releases once `history_retention_s` has passed since the
/// commit of the revision that superseded it, or when it is older than the
/// newest `max_revisions` revisions (oldest first). The head is always held. A
/// resolved candidate releases once `history_retention_s` has passed since its
/// resolution; an unresolved conflict candidate is never released.
pub fn retention_plan(
    core: &GraphCore,
    doc: &DocId,
    now_ms: u64,
    history_retention_s: u64,
    max_revisions: u64,
) -> RetentionPlan {
    let retention_ms = history_retention_s.saturating_mul(1000);
    let mut plan = RetentionPlan {
        held: Vec::new(),
        released: Vec::new(),
    };
    if let Some(head) = load_head(core, doc) {
        let first_kept = head.revision.saturating_sub(max_revisions) + 1;
        for revision in 1..=head.revision {
            let Some(row) = load_row::<RevisionRow>(core, &revision_node_id(doc, revision)) else {
                continue;
            };
            let superseded_at = if revision < head.revision {
                load_row::<RevisionRow>(core, &revision_node_id(doc, revision + 1))
                    .map(|next| next.committed_at_ms)
            } else {
                None
            };
            let past_deadline = superseded_at
                .is_some_and(|at| now_ms >= at.saturating_add(retention_ms));
            if revision < first_kept || past_deadline {
                plan.released.push((SnapshotSlot::Revision(revision), row.head.content_digest.to_hex()));
            } else {
                plan.held.push((SnapshotSlot::Revision(revision), row.content));
            }
        }
    }
    for (number, row) in candidates(core, doc) {
        let past_deadline = row.resolved
            && row
                .resolved_at_ms
                .is_some_and(|at| now_ms >= at.saturating_add(retention_ms));
        if past_deadline {
            plan.released.push((SnapshotSlot::Candidate(number), row.content_digest.to_hex()));
        } else {
            plan.held.push((SnapshotSlot::Candidate(number), row.content));
        }
    }
    plan
}

/// The bytes of content the graph holds for memory snapshots: every document's
/// head plus every retained candidate (MEDULLA-DURABLE-KERNEL-R085.7). A head
/// is the document's current bytes; earlier revisions release on the retention
/// rules and do not count against the tenant's capacity.
pub fn held_bytes(core: &GraphCore) -> u64 {
    let heads: u64 = core
        .get_nodes_by_label(HEAD_LABEL, usize::MAX)
        .into_iter()
        .filter_map(|(_, bytes)| decode_row::<RevisionRow>(&bytes))
        .map(|row| u64::from(row.head.content_len))
        .sum();
    let candidates: u64 = core
        .get_nodes_by_label(CANDIDATE_LABEL, usize::MAX)
        .into_iter()
        .filter_map(|(_, bytes)| decode_row::<CandidateRow>(&bytes))
        .filter(|row| !row.resolved)
        .map(|row| row.content.len() as u64)
        .sum();
    heads + candidates
}

/// Whether holding `incoming` more bytes would pass `capacity_bytes`.
pub fn exceeds_capacity(held: u64, incoming: u64, capacity_bytes: u64) -> bool {
    held.saturating_add(incoming) > capacity_bytes
}

/// Whether `core` still holds the pre-image `plan` was decided against: the same
/// head revision and the same number of retained candidates of `doc`.
pub fn plan_is_current(core: &GraphCore, doc: &DocId, plan: &SnapshotPlan) -> bool {
    load_head(core, doc).map(|head| head.revision) == plan.base_revision
        && candidates(core, doc).len() as u64 == plan.base_candidates
}

/// Write the rows of `plan`. The commit and durable replay both call this, so
/// both reach byte-identical rows.
pub fn apply_plan(core: &GraphCore, plan: &SnapshotPlan) {
    for write in &plan.writes {
        core.add_node(write.node_id.clone(), write.properties_msgpack.clone());
    }
}

#[cfg(test)]
mod retention_tests {
    use super::*;
    use medulla_mutation_types::memory_sync::{RequestId, SnapshotProvenance};

    fn put_revision(core: &GraphCore, doc: &DocId, revision: u64, committed_at_ms: u64, head: bool) {
        let content = format!("content-{revision}");
        let row = RevisionRow {
            label: REVISION_LABEL.to_string(),
            head: MemorySnapshotHead {
                doc_id: doc.clone(),
                revision,
                parent_revision: revision.checked_sub(1).filter(|p| *p > 0),
                content_digest: Digest256::sha256(content.as_bytes()),
                content_len: content.len() as u32,
                graph_incarnation: "g".into(),
                provenance: SnapshotProvenance {
                    actor: "a".into(),
                    policy_digest: Digest256::sha256(b"p"),
                    request_id: RequestId::new(&format!("r{revision}")).unwrap(),
                },
                retain_until_ms: None,
                doc_kind: Default::default(),
            },
            content,
            committed_at_ms,
        };
        core.add_node(revision_node_id(doc, revision), encode_row(&row));
        if head {
            core.add_node(head_node_id(doc), encode_row(&row));
        }
    }

    // spec: MEDULLA-DURABLE-KERNEL-R085.4
    #[test]
    fn revision_and_candidate_holders_never_share_an_id() {
        let doc = DocId::new("notes/a.md").unwrap();
        assert_eq!(SnapshotSlot::Revision(2).holder_id(&doc), "memory_snapshot:notes/a.md@2");
        assert_eq!(SnapshotSlot::Candidate(2).holder_id(&doc), "memory_snapshot:notes/a.md@c2");
    }

    // spec: MEDULLA-DURABLE-KERNEL-R085.7
    #[test]
    fn a_superseded_revision_releases_after_the_superseding_commit_plus_retention() {
        let core = GraphCore::new();
        let doc = DocId::new("notes/a.md").unwrap();
        put_revision(&core, &doc, 1, 1_000, false);
        put_revision(&core, &doc, 2, 5_000, true);
        let retention_s = 10;
        // Revision 1 was superseded at 5_000: the deadline is 15_000.
        let before = retention_plan(&core, &doc, 14_999, retention_s, 64);
        assert!(before.released.is_empty());
        assert_eq!(before.held.len(), 2);
        let after = retention_plan(&core, &doc, 15_000, retention_s, 64);
        assert_eq!(after.released.len(), 1);
        assert_eq!(after.released[0].0, SnapshotSlot::Revision(1));
        // The head never releases on a deadline.
        assert_eq!(after.held, vec![(SnapshotSlot::Revision(2), "content-2".to_string())]);
    }
}
