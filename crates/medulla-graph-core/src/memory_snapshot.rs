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

pub const HEAD_LABEL: &str = "MemorySnapshot";
pub const REVISION_LABEL: &str = "MemoryRevision";
pub const CANDIDATE_LABEL: &str = "MemoryCandidate";
pub const ACK_LABEL: &str = "MemoryAck";

/// A head or revision row: the head record and the bytes it stands for.
#[derive(Serialize, Deserialize)]
pub struct RevisionRow {
    pub label: String,
    pub head: MemorySnapshotHead,
    pub content: String,
}

/// A retained candidate row.
#[derive(Serialize, Deserialize)]
pub struct CandidateRow {
    pub label: String,
    pub base_revision: u64,
    pub content: String,
    pub content_digest: Digest256,
    pub resolved: bool,
}

/// An acknowledgment row.
#[derive(Serialize, Deserialize)]
pub struct AckRow {
    pub label: String,
    pub revision: u64,
    pub content_digest: Digest256,
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
