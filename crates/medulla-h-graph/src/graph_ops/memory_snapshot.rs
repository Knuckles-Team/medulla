//! Served memory snapshot reads, listing and sync (MEDULLA-DURABLE-KERNEL-R085.2,
//! R085.3, R085.4, R085.5).
//!
//! The scope check (`memory:read` / `memory:write`) runs at the request
//! boundary before this module is reached and refuses with
//! `MEMSYNC_NOT_AVAILABLE`. Every refusal here is decided before the commit
//! and writes nothing. The rows and the compare-and-set live in
//! `medulla_core::memory_snapshot_store`, shared with durable replay.

use super::memory_cursor;
use medulla_core::memory_snapshot_store as store;
use medulla_graph_core::graph::GraphCore;
use medulla_protocol_types::protocol::{Method, Response, ResultPayload};
use medulla_protocol_types::result_contract::graph as results;
use medulla_server_mutation::mutation::{MutationCtx, MutationPlan};
use medulla_server_ops::api::ingest::graph::commit_gateway;
use medulla_types::contract::Digest256;
use medulla_types::memory_sync::{
    decide, Decision, DocId, ListCursor, MemorySnapshotList, MemorySyncError, MemorySyncRefusal,
    SyncOutcome, SyncRequest, SyncStamp,
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
