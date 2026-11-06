//! Memory snapshot rows on the request graph (MEDULLA-DURABLE-KERNEL-R085.3,
//! R085.6).
//!
//! Layout: head `memsnap:<doc_id>`, revision `memrev:<doc_id>@<revision>`,
//! retained candidate `memcand:<doc_id>@<n>` and acknowledgment
//! `memack:<doc_id>`. [`apply_sync`] is the one mutation: the served handler
//! calls it inside the gateway commit and durable replay calls it again over
//! the same pre-image, so both reach byte-identical rows.

use medulla_graph_core::graph::GraphCore;
use medulla_types::contract::Digest256;
use medulla_types::memory_sync::{
    decide, Decision, DocId, MemorySnapshotHead, MemorySnapshotRead, MemorySyncRefusal, Resolution,
    SnapshotProvenance, SyncOutcome, SyncRequest, SyncStamp, SyncState,
};
use serde::{Deserialize, Serialize};

const HEAD_LABEL: &str = "MemorySnapshot";
const REVISION_LABEL: &str = "MemoryRevision";
const CANDIDATE_LABEL: &str = "MemoryCandidate";
const ACK_LABEL: &str = "MemoryAck";

#[derive(Serialize, Deserialize)]
struct RevisionRow {
    label: String,
    head: MemorySnapshotHead,
    content: String,
}

#[derive(Serialize, Deserialize)]
struct CandidateRow {
    label: String,
    base_revision: u64,
    content: String,
    content_digest: Digest256,
    resolved: bool,
}

#[derive(Serialize, Deserialize)]
struct AckRow {
    label: String,
    revision: u64,
    content_digest: Digest256,
    observation_generation: u64,
}

fn encode<T: Serialize>(row: &T) -> Vec<u8> {
    let value = serde_json::to_value(row).unwrap_or(serde_json::Value::Null);
    rmp_serde::to_vec_named(&value).unwrap_or_default()
}

fn decode<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> Option<T> {
    let value: serde_json::Value = rmp_serde::from_slice(bytes).ok()?;
    serde_json::from_value(value).ok()
}

/// The node id of a document's head.
pub fn head_node_id(doc: &DocId) -> String {
    format!("memsnap:{}", doc.as_str())
}

fn revision_node_id(doc: &DocId, revision: u64) -> String {
    format!("memrev:{}@{revision}", doc.as_str())
}

fn candidate_prefix(doc: &DocId) -> String {
    format!("memcand:{}@", doc.as_str())
}

fn ack_node_id(doc: &DocId) -> String {
    format!("memack:{}", doc.as_str())
}

fn load_row<T: for<'de> Deserialize<'de>>(core: &GraphCore, id: &str) -> Option<T> {
    core.get_node_properties(id)
        .and_then(|bytes| decode(&bytes))
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
        .filter_map(|(_, bytes)| decode::<RevisionRow>(&bytes))
        .map(|row| row.head)
        .filter(|head| head.doc_id.as_str().starts_with(prefix))
        .collect();
    heads.sort_by(|a, b| a.doc_id.cmp(&b.doc_id));
    heads
}

fn candidates(core: &GraphCore, doc: &DocId) -> Vec<(u64, CandidateRow)> {
    let prefix = candidate_prefix(doc);
    core.get_nodes_by_label(CANDIDATE_LABEL, usize::MAX)
        .into_iter()
        .filter_map(|(id, bytes)| {
            let n = id.strip_prefix(&prefix)?.parse::<u64>().ok()?;
            Some((n, decode::<CandidateRow>(&bytes)?))
        })
        .collect()
}

fn write_revision(
    core: &GraphCore,
    request: &SyncRequest,
    stamp: &SyncStamp,
    revision: u64,
    parent_revision: Option<u64>,
    content: String,
    content_digest: Digest256,
) -> MemorySnapshotHead {
    let head = MemorySnapshotHead {
        doc_id: request.doc_id.clone(),
        revision,
        parent_revision,
        content_digest,
        content_len: u32::try_from(content.len()).unwrap_or(u32::MAX),
        graph_incarnation: stamp.graph_incarnation.clone(),
        provenance: SnapshotProvenance {
            actor: stamp.actor.clone(),
            policy_digest: stamp.policy_digest,
            request_id: request.request_id.clone(),
        },
        retain_until_ms: None,
    };
    let row = RevisionRow {
        label: REVISION_LABEL.to_string(),
        head: head.clone(),
        content,
    };
    core.add_node(revision_node_id(&request.doc_id, revision), encode(&row));
    let head_row = RevisionRow {
        label: HEAD_LABEL.to_string(),
        ..row
    };
    core.add_node(head_node_id(&request.doc_id), encode(&head_row));
    head
}

fn refused(code: MemorySyncRefusal) -> SyncOutcome {
    SyncOutcome::Refused { code }
}

fn retain_candidate(
    core: &GraphCore,
    doc: &DocId,
    base_revision: u64,
    content: String,
    content_digest: Digest256,
) -> u64 {
    let n = candidates(core, doc)
        .iter()
        .map(|(n, _)| *n)
        .max()
        .unwrap_or(0)
        + 1;
    let row = CandidateRow {
        label: CANDIDATE_LABEL.to_string(),
        base_revision,
        content,
        content_digest,
        resolved: false,
    };
    core.add_node(format!("{}{n}", candidate_prefix(doc)), encode(&row));
    n
}

fn resolve(
    core: &GraphCore,
    request: &SyncRequest,
    stamp: &SyncStamp,
    candidate_revision: u64,
    resolution: Resolution,
    new_revision: u64,
) -> SyncOutcome {
    let doc = &request.doc_id;
    let candidate_id = format!("{}{candidate_revision}", candidate_prefix(doc));
    let Some(mut candidate) = load_row::<CandidateRow>(core, &candidate_id).filter(|c| !c.resolved)
    else {
        return refused(MemorySyncRefusal::NotAvailable);
    };
    let Some(head) = load_head(core, doc) else {
        return refused(MemorySyncRefusal::NotAvailable);
    };
    candidate.resolved = true;
    core.add_node(candidate_id, encode(&candidate));
    let (content, digest) = match resolution {
        Resolution::KeepHead => return SyncOutcome::Resolved { head },
        Resolution::TakeCandidate => (candidate.content, candidate.content_digest),
        Resolution::Merged {
            content,
            content_digest,
        } => (content, content_digest),
    };
    let head = write_revision(
        core,
        request,
        stamp,
        new_revision,
        Some(head.revision),
        content,
        digest,
    );
    SyncOutcome::Resolved { head }
}

fn acknowledge(core: &GraphCore, request: &SyncRequest, state: SyncState) -> SyncOutcome {
    let medulla_types::memory_sync::SyncAction::Acknowledge {
        revision,
        content_digest,
        observation_generation,
    } = &request.action
    else {
        return refused(MemorySyncRefusal::VersionMalformed);
    };
    let doc = &request.doc_id;
    let unresolved = candidates(core, doc).iter().any(|(_, c)| !c.resolved);
    let stale = load_row::<AckRow>(core, &ack_node_id(doc))
        .is_some_and(|held| held.observation_generation > *observation_generation);
    let state = match (unresolved, stale, state) {
        (true, _, _) => SyncState::Conflict,
        (false, true, _) => SyncState::Pending,
        (false, false, state) => state,
    };
    let row = AckRow {
        label: ACK_LABEL.to_string(),
        revision: *revision,
        content_digest: *content_digest,
        observation_generation: *observation_generation,
    };
    core.add_node(ack_node_id(doc), encode(&row));
    SyncOutcome::Acknowledged { state }
}

/// Decide `request` against the stored head and write what the decision
/// names. A reply or refusal writes nothing.
pub fn apply_sync(core: &GraphCore, request: &SyncRequest, stamp: &SyncStamp) -> SyncOutcome {
    let head = load_head(core, &request.doc_id);
    match decide(head.as_ref(), request) {
        Decision::Reply(outcome) => outcome,
        Decision::Accept {
            revision,
            parent_revision,
            content,
            content_digest,
        } => SyncOutcome::Accepted {
            head: write_revision(
                core,
                request,
                stamp,
                revision,
                parent_revision,
                content,
                content_digest,
            ),
        },
        Decision::RetainCandidate {
            content,
            content_digest,
            head_revision,
        } => {
            let candidate_revision = retain_candidate(
                core,
                &request.doc_id,
                head_revision,
                content,
                content_digest,
            );
            match head {
                Some(head) => SyncOutcome::ConflictRetained {
                    head,
                    candidate_revision,
                },
                None => refused(MemorySyncRefusal::NotAvailable),
            }
        }
        Decision::Resolve {
            candidate_revision,
            resolution,
            new_revision,
        } => resolve(
            core,
            request,
            stamp,
            candidate_revision,
            resolution,
            new_revision,
        ),
        Decision::Acknowledge { state } => acknowledge(core, request, state),
    }
}
