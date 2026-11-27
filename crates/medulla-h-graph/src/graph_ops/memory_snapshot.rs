//! Served memory snapshot reads, listing and sync (MEDULLA-DURABLE-KERNEL-R085.2,
//! R085.3, R085.4, R085.5).
//!
//! The scope check (`memory:read` / `memory:write`) runs at the request
//! boundary before this module is reached and refuses with
//! `MEMSYNC_NOT_AVAILABLE`. Every refusal here is decided before the commit
//! and writes nothing. A sync is planned, then applied: [`decide_sync`] decides
//! against the stored rows and returns the outcome with the rows it writes;
//! the router arm gates it and hands the plan back on the method, and
//! [`sync`] commits it through the gateway only over the pre-image it was
//! decided against. The row layout and reads live in
//! `medulla_graph_core::memory_snapshot`, shared with the decide handler.

use super::memory_cursor;
use medulla_graph_core::graph::GraphCore;
use medulla_graph_core::memory_snapshot as store;
use medulla_graph_core::memory_snapshot::{AckRow, CandidateRow, RevisionRow, SnapshotSlot};
use medulla_mutation_types::memory_sync::{
    decide, revision_node_id, Decision, DocId, ListCursor, MemoryDocKind, MemorySnapshotHead,
    MemorySnapshotList, MemorySnapshotRead, MemorySyncError, MemorySyncRefusal, Resolution,
    SearchStateEvidence, SnapshotPlan, SnapshotProvenance, SnapshotRowWrite, SyncAction,
    SyncOutcome, SyncReceipt, SyncRequest, SyncStamp, SyncStanding,
};
use medulla_protocol_types::protocol::{Method, Response, ResultPayload};
use medulla_protocol_types::result_contract::graph as results;
use medulla_server_mutation::mutation::{MutationCtx, MutationPlan};
use medulla_server_ops::api::ingest::graph::commit_gateway;
use medulla_types::contract::Digest256;
use medulla_work_types::memory::{validate_search_state, MemoryError, SearchStateEnvelope};

/// `limit` default and maximum of `ListMemorySnapshots`.
const DEFAULT_LIMIT: u32 = 50;
const MAX_LIMIT: u32 = 200;

fn refuse(req_id: u64, code: MemorySyncRefusal) -> Response {
    Response::err(req_id, code.as_str())
}

fn payload(req_id: u64, body: Result<ResultPayload, String>) -> Response {
    match body {
        Ok(payload) => Response::ok(req_id, payload),
        Err(error) => Response::err(req_id, error),
    }
}

fn parse_doc(req_id: u64, doc_id: &str) -> Result<DocId, Box<Response>> {
    DocId::new(doc_id).map_err(|MemorySyncError { code, .. }| Box::new(refuse(req_id, code)))
}

/// `ReadMemorySnapshot`: the head or a retained revision with its bytes.
pub(crate) fn read(req_id: u64, core: &GraphCore, doc_id: &str, revision: Option<u64>) -> Response {
    let doc = match parse_doc(req_id, doc_id) {
        Ok(doc) => doc,
        Err(response) => return *response,
    };
    match store::read(core, &doc, revision).map(with_search_state) {
        Some(found) => payload(
            req_id,
            ResultPayload::of::<results::ReadMemorySnapshot>(found),
        ),
        None => refuse(req_id, MemorySyncRefusal::NotAvailable),
    }
}

/// Attach the derived evidence of a saved search state to its read
/// (MEDULLA-DECISION-ENGINE-R149.5). A body that no longer parses as a search
/// state carries no evidence; nothing is invented.
fn with_search_state(mut read: MemorySnapshotRead) -> MemorySnapshotRead {
    if read.head.doc_kind != MemoryDocKind::SearchState {
        return read;
    }
    let Ok(envelope) = serde_json::from_str::<SearchStateEnvelope>(&read.content) else {
        return read;
    };
    let hex = [
        envelope.transcript_digest().to_hex(),
        envelope.evidence_digest().to_hex(),
        envelope.constraints_digest().to_hex(),
        envelope.open_claims_digest().to_hex(),
    ];
    let premises = vec![
        format!("search:run={}", envelope.search_run_id),
        format!("search:revision={}", envelope.state_revision),
        format!("search:transcript={}", hex[0]),
        format!("search:evidence={}", hex[1]),
        format!("search:constraints={}", hex[2]),
        format!("search:open_claims={}", hex[3]),
    ];
    let [transcript_digest, evidence_digest, constraints_digest, open_claims_digest] = hex;
    read.search_state = Some(SearchStateEvidence {
        search_run_id: envelope.search_run_id,
        state_revision: envelope.state_revision,
        transcript_digest,
        evidence_digest,
        constraints_digest,
        open_claims_digest,
        recovered_at_ms: medulla_kernel::kernel::dispatch_support::authoritative_now_ms(),
        premises,
    });
    read
}

/// Refuse a `SearchState` document whose planned revision is not a valid
/// search state (MEDULLA-DECISION-ENGINE-R149.3, R149.4). Only an outcome that
/// writes a new revision is checked: a conflict keeps the head and returns the
/// R085 outcome, and every refusal here is decided before the commit, so the
/// head is unchanged.
pub fn check_doc_kind(
    request: &SyncRequest,
    outcome: &SyncOutcome,
    plan: &SnapshotPlan,
) -> Result<(), String> {
    if request.doc_kind != MemoryDocKind::SearchState {
        return Ok(());
    }
    let (SyncOutcome::Accepted { head } | SyncOutcome::Resolved { head }) = outcome else {
        return Ok(());
    };
    let Some((_, content)) = planned_content(plan, &request.doc_id, outcome) else {
        return Ok(());
    };
    let max = super::memory_surface::search_state_max_bytes();
    if content.len() as u64 > max {
        return Err(MemoryError::SearchStateTooLarge(format!(
            "body is {} bytes, above knob memory.search_state_max_bytes ({max})",
            content.len()
        ))
        .to_string());
    }
    validate_search_state(
        content.as_bytes(),
        head.revision,
        head.parent_revision.unwrap_or(0),
    )
    .map(|_| ())
    .map_err(|error| error.to_string())
}

/// Wall-clock seconds, the clock cursor expiry is judged on.
pub(crate) fn now_s() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

/// The caller and filter one `ListMemorySnapshots` page is bound to.
pub(crate) struct ListScope<'a> {
    pub secret: &'a str,
    pub authority: &'a medulla_server_api::carrier::GraphReadAuthority,
    pub graph_name: &'a str,
    pub now_s: u64,
}

/// `ListMemorySnapshots`: heads in `doc_id` order, at most `limit`, resumed
/// after the signed cursor's `doc_id` (R085.2). The page reads the caller's
/// visible view only, so it carries no hidden count, id or digest.
pub(crate) fn list(
    req_id: u64,
    core: &GraphCore,
    scope: &ListScope<'_>,
    prefix: &str,
    limit: Option<u32>,
    cursor: Option<&str>,
) -> Response {
    let limit = limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    match list_page(core, scope, prefix, limit, cursor) {
        Ok(page) => payload(
            req_id,
            ResultPayload::of::<results::ListMemorySnapshots>(page),
        ),
        Err(code) => refuse(req_id, code),
    }
}

fn list_page(
    core: &GraphCore,
    scope: &ListScope<'_>,
    prefix: &str,
    limit: u32,
    cursor: Option<&str>,
) -> Result<MemorySnapshotList, MemorySyncRefusal> {
    let carrier = scope
        .authority
        .carrier()
        .ok_or(MemorySyncRefusal::NotAvailable)?;
    let binding = memory_cursor::CursorBinding {
        tenant: carrier.tenant_scope(),
        graph_incarnation: scope.graph_name,
        principal: carrier.agent_id(),
        policy_digest: carrier.authority_digest(),
        prefix,
        limit,
    };
    let ttl_s = medulla_server_api::knob_config::knobs().memory_sync_cursor_ttl_s;
    let after = cursor
        .map(|token| memory_cursor::decode(scope.secret, &binding, token, scope.now_s, ttl_s))
        .transpose()?;
    let mut heads: Vec<_> = store::list_heads(core, prefix)
        .into_iter()
        .filter(|head| {
            after
                .as_deref()
                .is_none_or(|last| head.doc_id.as_str() > last)
        })
        .take(limit as usize + 1)
        .collect();
    let more = heads.len() > limit as usize;
    heads.truncate(limit as usize);
    let next_cursor = match (more, heads.last()) {
        (true, Some(last)) => Some(
            ListCursor::new(memory_cursor::encode(
                scope.secret,
                &binding,
                last.doc_id.as_str(),
                scope.now_s,
            ))
            .map_err(|error| error.code)?,
        ),
        _ => None,
    };
    Ok(MemorySnapshotList { heads, next_cursor })
}

pub fn outcome_response(req_id: u64, outcome: SyncOutcome, warnings: Vec<String>) -> Response {
    match outcome {
        SyncOutcome::Refused { code } => refuse(req_id, code),
        other => payload(
            req_id,
            ResultPayload::of::<results::SyncMemorySnapshot>(SyncReceipt {
                outcome: other,
                warnings,
            }),
        ),
    }
}

/// The engine stamp of a sync made by `caller` on `graph_name` of `tenant_scope`.
pub fn stamp_for(caller: Option<&str>, tenant_scope: &str, graph_name: &str) -> SyncStamp {
    let policy = format!("{tenant_scope}|{graph_name}|memory:write");
    SyncStamp {
        actor: caller.unwrap_or("anonymous").to_string(),
        policy_digest: Digest256::sha256(policy.as_bytes()),
        graph_incarnation: graph_name.to_string(),
        committed_at_ms: medulla_kernel::kernel::dispatch_support::authoritative_now_ms(),
    }
}

fn write_of<T: serde::Serialize>(node_id: String, row: &T) -> SnapshotRowWrite {
    SnapshotRowWrite {
        node_id,
        properties_msgpack: store::encode_row(row),
    }
}

/// The revision row and the head row of one accepted or resolved revision.
fn revision_writes(
    request: &SyncRequest,
    stamp: &SyncStamp,
    revision: u64,
    parent_revision: Option<u64>,
    content: String,
    content_digest: Digest256,
) -> (MemorySnapshotHead, Vec<SnapshotRowWrite>) {
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
        doc_kind: request.doc_kind,
    };
    let row = RevisionRow {
        label: store::REVISION_LABEL.to_string(),
        head: head.clone(),
        content,
        committed_at_ms: stamp.committed_at_ms,
    };
    let revision_write = write_of(revision_node_id(&request.doc_id, revision), &row);
    let head_write = write_of(
        store::head_node_id(&request.doc_id),
        &RevisionRow {
            label: store::HEAD_LABEL.to_string(),
            ..row
        },
    );
    (head, vec![revision_write, head_write])
}

fn refused(code: MemorySyncRefusal) -> SyncOutcome {
    SyncOutcome::Refused { code }
}

fn plan_resolve(
    core: &GraphCore,
    request: &SyncRequest,
    stamp: &SyncStamp,
    candidate_revision: u64,
    resolution: Resolution,
    new_revision: u64,
) -> (SyncOutcome, Vec<SnapshotRowWrite>) {
    let doc = &request.doc_id;
    let candidate_id = format!("{}{candidate_revision}", store::candidate_prefix(doc));
    let Some(mut candidate) =
        store::load_row::<CandidateRow>(core, &candidate_id).filter(|c| !c.resolved)
    else {
        return (refused(MemorySyncRefusal::NotAvailable), Vec::new());
    };
    let Some(head) = store::load_head(core, doc) else {
        return (refused(MemorySyncRefusal::NotAvailable), Vec::new());
    };
    candidate.resolved = true;
    candidate.resolved_at_ms = Some(stamp.committed_at_ms);
    let mut writes = vec![write_of(candidate_id, &candidate)];
    let (content, digest) = match resolution {
        Resolution::KeepHead => return (SyncOutcome::Resolved { head }, writes),
        Resolution::TakeCandidate => (candidate.content, candidate.content_digest),
        Resolution::Merged {
            content,
            content_digest,
        } => (content, content_digest),
    };
    let (head, rows) = revision_writes(
        request,
        stamp,
        new_revision,
        Some(head.revision),
        content,
        digest,
    );
    writes.extend(rows);
    (SyncOutcome::Resolved { head }, writes)
}

fn plan_acknowledge(
    core: &GraphCore,
    request: &SyncRequest,
    standing: SyncStanding,
) -> (SyncOutcome, Vec<SnapshotRowWrite>) {
    let SyncAction::Acknowledge {
        revision,
        content_digest,
        observation_generation,
    } = &request.action
    else {
        return (refused(MemorySyncRefusal::VersionMalformed), Vec::new());
    };
    let doc = &request.doc_id;
    let unresolved = store::candidates(core, doc)
        .iter()
        .any(|(_, c)| !c.resolved);
    let stale = store::load_row::<AckRow>(core, &store::ack_node_id(doc))
        .is_some_and(|held| held.observation_generation > *observation_generation);
    let standing = match (unresolved, stale, standing) {
        (true, _, _) => SyncStanding::Conflict,
        (false, _, SyncStanding::Unavailable) => SyncStanding::Unavailable,
        (false, true, _) => SyncStanding::Pending,
        (false, false, standing) => standing,
    };
    let row = AckRow {
        label: store::ACK_LABEL.to_string(),
        revision: *revision,
        content_digest: *content_digest,
        observation_generation: *observation_generation,
    };
    (
        SyncOutcome::Acknowledged { standing },
        vec![write_of(store::ack_node_id(doc), &row)],
    )
}

/// Decide `request` against the stored rows and return the outcome with the rows
/// it writes, bound to the pre-image it was decided against. A reply or refusal
/// plans no row.
/// `head_content_held` is the router's pre-commit CAS answer for the head's
/// content: when it is false a standing reading is `Unavailable`
/// (MEDULLA-DURABLE-KERNEL-R085.4, R875.1).
pub fn decide_sync(
    core: &GraphCore,
    request: &SyncRequest,
    stamp: &SyncStamp,
    head_content_held: bool,
) -> (SyncOutcome, SnapshotPlan) {
    let head = store::load_head(core, &request.doc_id);
    let candidate_rows = store::candidates(core, &request.doc_id);
    let (outcome, writes) = match decide(head.as_ref(), request) {
        Decision::Reply(SyncOutcome::Observed {
            head_revision,
            head_digest,
            standing,
        }) => (
            SyncOutcome::Observed {
                head_revision,
                head_digest,
                standing: if head_content_held {
                    standing
                } else {
                    SyncStanding::Unavailable
                },
            },
            Vec::new(),
        ),
        Decision::Reply(outcome) => (outcome, Vec::new()),
        Decision::Accept {
            revision,
            parent_revision,
            content,
            content_digest,
        } => {
            let (head, writes) = revision_writes(
                request,
                stamp,
                revision,
                parent_revision,
                content,
                content_digest,
            );
            (SyncOutcome::Accepted { head }, writes)
        }
        Decision::RetainCandidate {
            content,
            content_digest,
            head_revision,
        } => match head.clone() {
            None => (refused(MemorySyncRefusal::NotAvailable), Vec::new()),
            Some(head) => {
                let candidate_revision =
                    candidate_rows.iter().map(|(n, _)| *n).max().unwrap_or(0) + 1;
                let row = CandidateRow {
                    label: store::CANDIDATE_LABEL.to_string(),
                    base_revision: head_revision,
                    content,
                    content_digest,
                    resolved: false,
                    resolved_at_ms: None,
                };
                let id = format!(
                    "{}{candidate_revision}",
                    store::candidate_prefix(&request.doc_id)
                );
                (
                    SyncOutcome::ConflictRetained {
                        head,
                        candidate_revision,
                    },
                    vec![write_of(id, &row)],
                )
            }
        },
        Decision::Resolve {
            candidate_revision,
            resolution,
            new_revision,
        } => plan_resolve(
            core,
            request,
            stamp,
            candidate_revision,
            resolution,
            new_revision,
        ),
        Decision::Acknowledge { standing } => plan_acknowledge(
            core,
            request,
            if head_content_held {
                standing
            } else {
                SyncStanding::Unavailable
            },
        ),
    };
    (
        outcome,
        SnapshotPlan {
            base_revision: head.map(|head| head.revision),
            base_candidates: candidate_rows.len() as u64,
            writes,
        },
    )
}

/// The hex digest of the accepted head's content, the CAS key the router checks
/// before it decides a standing reading. `None` for an absent document.
pub fn head_content_digest(core: &GraphCore, doc: &DocId) -> Option<String> {
    store::load_head(core, doc).map(|head| head.content_digest.to_hex())
}

/// The change a planned sync makes to existing content, as the admission gate
/// names it: the new content digest and the head revision it replaces. A submit
/// over a head is a rewrite and a resolution that writes a revision is a
/// consolidation; every other sync changes no existing content and needs no
/// receipt (MEDULLA-DURABLE-KERNEL-R863).
pub fn content_change(
    request: &SyncRequest,
    outcome: &SyncOutcome,
    plan: &SnapshotPlan,
) -> Option<(ContentChange, Digest256, u64)> {
    let head_revision = plan.base_revision?;
    match (&request.action, outcome) {
        (SyncAction::Submit { .. }, SyncOutcome::Accepted { head }) => {
            Some((ContentChange::Rewrite, head.content_digest, head_revision))
        }
        (SyncAction::Resolve { .. }, SyncOutcome::Resolved { head })
            if head.revision > head_revision =>
        {
            Some((
                ContentChange::Consolidate,
                head.content_digest,
                head_revision,
            ))
        }
        _ => None,
    }
}

/// The slot (revision or candidate) and bytes a planned sync will store in the graph and that
/// the blob holder must keep (MEDULLA-DURABLE-KERNEL-R085.7): the new head or
/// retained revision after `Accepted` or `Resolved`, the retained candidate
/// after `ConflictRetained`. `None` for an outcome that stores no content. It
/// reads the plan, so the content can reach the CAS before the graph commit.
pub fn planned_content(
    plan: &SnapshotPlan,
    doc: &DocId,
    outcome: &SyncOutcome,
) -> Option<(SnapshotSlot, String)> {
    let written = |node_id: String| {
        plan.writes
            .iter()
            .find(|write| write.node_id == node_id)
            .map(|write| write.properties_msgpack.as_slice())
    };
    match outcome {
        SyncOutcome::Accepted { head } | SyncOutcome::Resolved { head } => {
            let bytes = written(revision_node_id(doc, head.revision))?;
            store::decode_row::<RevisionRow>(bytes)
                .map(|row| (SnapshotSlot::Revision(head.revision), row.content))
        }
        SyncOutcome::ConflictRetained {
            candidate_revision, ..
        } => {
            let bytes = written(format!(
                "{}{candidate_revision}",
                store::candidate_prefix(doc)
            ))?;
            store::decode_row::<CandidateRow>(bytes)
                .map(|row| (SnapshotSlot::Candidate(*candidate_revision), row.content))
        }
        _ => None,
    }
}

/// Whether storing what `plan` writes would hold more than the tenant's
/// `capacity_bytes` of memory snapshot content (MEDULLA-DURABLE-KERNEL-R085.7).
/// A new head replaces the head it supersedes; a retained candidate adds to it.
/// The router refuses `MEMSYNC_RETENTION_EXHAUSTED` before any effect.
pub fn exceeds_capacity(
    core: &GraphCore,
    doc: &DocId,
    outcome: &SyncOutcome,
    plan: &SnapshotPlan,
    capacity_bytes: u64,
) -> bool {
    let Some((_, content)) = planned_content(plan, doc, outcome) else {
        return false;
    };
    let incoming = content.len() as u64;
    let replaced = match outcome {
        SyncOutcome::Accepted { .. } | SyncOutcome::Resolved { .. } => {
            store::load_head(core, doc).map_or(0, |head| u64::from(head.content_len))
        }
        _ => 0,
    };
    store::exceeds_capacity(
        store::held_bytes(core).saturating_sub(replaced),
        incoming,
        capacity_bytes,
    )
}

/// Which gate kind a content change maps to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentChange {
    Rewrite,
    Consolidate,
}

/// Commit a planned sync through the gateway. The router planned the rows and
/// ran the receipt gate before it named `SyncMemorySnapshotStore`. The plan
/// applies only over the pre-image it was decided against, so a head that moved
/// between plan and commit answers `BUSY` and the router plans again.
pub(crate) async fn sync(ctx: &MutationCtx<'_>, plan: &MutationPlan, method: &Method) -> Response {
    let Method::SyncMemorySnapshotStore {
        request,
        plan: snapshot,
    } = method
    else {
        return Response::err(
            ctx.req_id,
            "INTERNAL: a memory sync reached the graph route without its plan".to_string(),
        );
    };
    let doc = request.doc_id.clone();
    let snapshot = (**snapshot).clone();
    commit_gateway(ctx, plan, method, move |core| {
        if !store::plan_is_current(core, &doc, &snapshot) {
            return Err("BUSY: the memory snapshot moved between plan and commit".to_string());
        }
        store::apply_plan(core, &snapshot);
        Ok(ResultPayload::Bool(true))
    })
    .await
}
