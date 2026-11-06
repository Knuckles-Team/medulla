//! Served memory snapshot reads, listing and sync (MEDULLA-DURABLE-KERNEL-R085.2,
//! R085.3, R085.4, R085.5).
//!
//! The scope check (`memory:read` / `memory:write`) runs at the request
//! boundary before this module is reached and refuses with
//! `MEMSYNC_NOT_AVAILABLE`. Every refusal here is decided before the commit
//! and writes nothing. The rows and the compare-and-set live in
//! `medulla_core::memory_snapshot_store`, shared with durable replay.

use medulla_core::memory_snapshot_store as store;
use medulla_graph_core::graph::GraphCore;
use medulla_protocol_types::protocol::{Method, Response, ResultPayload};
use medulla_protocol_types::result_contract::graph as results;
use medulla_server_mutation::mutation::{MutationCtx, MutationPlan};
use medulla_server_ops::api::ingest::graph::commit_gateway;
use medulla_types::contract::Digest256;
use medulla_types::memory_sync::{
    decide, Decision, DocId, MemorySnapshotList, MemorySyncError, MemorySyncRefusal, SyncOutcome,
    SyncRequest, SyncStamp,
};

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

fn parse_doc(req_id: u64, doc_id: &str) -> Result<DocId, Response> {
    DocId::new(doc_id).map_err(|MemorySyncError { code, .. }| refuse(req_id, code))
}

/// `ReadMemorySnapshot`: the head or a retained revision with its bytes.
pub(crate) fn read(req_id: u64, core: &GraphCore, doc_id: &str, revision: Option<u64>) -> Response {
    let doc = match parse_doc(req_id, doc_id) {
        Ok(doc) => doc,
        Err(response) => return response,
    };
    match store::read(core, &doc, revision) {
        Some(found) => payload(
            req_id,
            ResultPayload::of::<results::ReadMemorySnapshot>(found),
        ),
        None => refuse(req_id, MemorySyncRefusal::NotAvailable),
    }
}

/// `ListMemorySnapshots`: heads in `doc_id` order, at most `limit`. A cursor
/// needs the signed token of R085.2 and is refused until that row lands.
pub(crate) fn list(
    req_id: u64,
    core: &GraphCore,
    prefix: &str,
    limit: Option<u32>,
    cursor: Option<&str>,
) -> Response {
    if cursor.is_some() {
        return refuse(req_id, MemorySyncRefusal::CursorInvalid);
    }
    let limit = limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT) as usize;
    let mut heads = store::list_heads(core, prefix);
    heads.truncate(limit);
    payload(
        req_id,
        ResultPayload::of::<results::ListMemorySnapshots>(MemorySnapshotList {
            heads,
            next_cursor: None,
        }),
    )
}

fn outcome_response(req_id: u64, outcome: SyncOutcome) -> Response {
    match outcome {
        SyncOutcome::Refused { code } => refuse(req_id, code),
        other => payload(
            req_id,
            ResultPayload::of::<results::SyncMemorySnapshot>(other),
        ),
    }
}

fn stamp_of(ctx: &MutationCtx<'_>) -> SyncStamp {
    let policy = format!("{}|{}|memory:write", ctx.tenant_scope, ctx.graph_name);
    SyncStamp {
        actor: ctx.caller.unwrap_or("anonymous").to_string(),
        policy_digest: Digest256::sha256(policy.as_bytes()),
        graph_incarnation: ctx.graph_name.to_string(),
    }
}

/// `SyncMemorySnapshot`: validate, decide against the head, then commit the
/// write the decision names through the gateway. A reply or refusal commits
/// nothing.
pub(crate) async fn sync(
    ctx: &MutationCtx<'_>,
    plan: &MutationPlan,
    request: &SyncRequest,
) -> Response {
    let head = store::load_head(ctx.core, &request.doc_id);
    if let Decision::Reply(outcome) = decide(head.as_ref(), request) {
        return outcome_response(ctx.req_id, outcome);
    }
    let stamp = stamp_of(ctx);
    let stamped = Method::SyncMemorySnapshot {
        request: Box::new(request.clone()),
        stamp: Some(stamp.clone()),
    };
    let request = request.clone();
    commit_gateway(ctx, plan, &stamped, move |core| {
        ResultPayload::of::<results::SyncMemorySnapshot>(store::apply_sync(core, &request, &stamp))
    })
    .await
}
